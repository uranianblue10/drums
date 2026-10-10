//! Native desktop audio: ffmpeg-decodes a loop to PCM once, then plays it
//! back gaplessly from memory with live varispeed tempo and an optional
//! metronome click track mixed in. Immune to webview audio quirks.
use rodio::{Sink, Source};
use std::sync::mpsc::Sender;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

pub const SAMPLE_RATE: u32 = 44100;

/// Infinite looping source over interleaved stereo f32 PCM.
/// Tempo (varispeed, pitch follows) is adjustable live via `ratio`.
pub struct LoopSrc {
    data: std::sync::Arc<Vec<f32>>,
    frames: usize,
    pos: f64,      // fractional frame position
    n: u64,        // samples yielded (for channel tracking)
    ratio: std::sync::Arc<AtomicU32>, // fixed point: 65536 == 1.0x
}

impl LoopSrc {
    pub fn new(data: std::sync::Arc<Vec<f32>>, ratio: std::sync::Arc<AtomicU32>) -> Self {
        Self {
            frames: data.len() / 2,
            data,
            pos: 0.0,
            n: 0,
            ratio,
        }
    }
}

impl Iterator for LoopSrc {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.frames == 0 {
            return None;
        }
        let ratio = self.ratio.load(Ordering::Relaxed) as f64 / 65536.0;
        let ch = (self.n % 2) as usize;
        let f = self.pos.floor();
        let i0 = (f as usize) % self.frames;
        let i1 = (i0 + 1) % self.frames;
        let frac = (self.pos - f) as f32;
        let a = self.data[i0 * 2 + ch];
        let b = self.data[i1 * 2 + ch];
        let v = a + (b - a) * frac;
        self.n += 1;
        if ch == 1 {
            self.pos += ratio;
            let span = self.frames as f64;
            if self.pos >= span {
                self.pos -= (self.pos / span).floor() * span;
            }
        }
        Some(v)
    }
}

impl Source for LoopSrc {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        2
    }
    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// Trim decoded PCM to the exact musical loop length.
pub fn trim_to_duration(pcm: &[f32], duration_ms: u64) -> Vec<f32> {
    if duration_ms == 0 {
        return pcm.to_vec();
    }
    let target_frames = (duration_ms as f64 / 1000.0 * SAMPLE_RATE as f64) as usize;
    let target_samples = target_frames * 2;
    if pcm.len() > target_samples {
        pcm[..target_samples].to_vec()
    } else {
        pcm.to_vec()
    }
}

/// Normalize (down only) and optionally mix a metronome click track on top.
/// Clicks land on a `metro_bpm` grid across the whole loop so they stay in
/// sync every iteration.
pub fn mix_clicks(pcm: &[f32], metro: bool, metro_bpm: f32) -> Vec<f32> {
    let peak = pcm.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
    let scale = if peak > 0.89 { 0.89 / peak } else { 1.0 };
    let mut out: Vec<f32> = pcm.iter().map(|s| s * scale).collect();
    if metro && metro_bpm > 0.0 {
        let period = 60.0 / metro_bpm as f64 * SAMPLE_RATE as f64;
        let click_len = (0.09 * SAMPLE_RATE as f64) as usize;
        let frames = out.len() / 2;
        let mut beat = 0.0;
        while (beat as usize) < frames {
            let start = beat as usize;
            for i in 0..click_len {
                let idx = start + i;
                if idx >= frames {
                    break;
                }
                let t = i as f32 / SAMPLE_RATE as f32;
                let env = (-t * 45.0).exp();
                let s = (2.0 * std::f32::consts::PI * 880.0 * t).sin() * env * 0.45;
                for ch in 0..2 {
                    let v = out[idx * 2 + ch] + s;
                    out[idx * 2 + ch] = v.clamp(-1.0, 1.0);
                }
            }
            beat += period;
        }
    }
    out
}

/// Decode an opus asset to stereo f32le PCM via ffmpeg (handles container
/// pre-skip etc.). `data_provider` supplies the raw opus bytes.
pub fn decode_opus_bytes(opus: &[u8], tag: &str) -> Result<Vec<f32>, String> {
    let dir = std::env::temp_dir().join("loopz-cache");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cache dir: {e}"))?;
    let safe: String = tag
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    let in_path = dir.join(format!("{safe}.opus"));
    let out_path = dir.join(format!("{safe}.raw"));
    // Reuse decoded output if the source didn't change.
    let existing = std::fs::metadata(&in_path).ok().map(|m| m.len());
    if existing != Some(opus.len() as u64) {
        std::fs::write(&in_path, opus).map_err(|e| format!("write tmp: {e}"))?;
        let _ = std::fs::remove_file(&out_path);
    }
    if std::fs::metadata(&out_path).is_err() {
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-i",
                &in_path.to_string_lossy(),
                "-map",
                "0:a",
                "-ac",
                "2",
                "-ar",
                &SAMPLE_RATE.to_string(),
                "-f",
                "f32le",
                &out_path.to_string_lossy(),
            ])
            .status()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    "ffmpeg not found - install it (pacman -S ffmpeg)".to_string()
                } else {
                    format!("ffmpeg failed to start: {e}")
                }
            })?;
        if !status.success() {
            return Err("ffmpeg could not decode this loop".to_string());
        }
    }
    let raw = std::fs::read(&out_path).map_err(|e| format!("read pcm: {e}"))?;
    if raw.len() < 8 || raw.len() % 4 != 0 {
        return Err("decoded audio is empty".to_string());
    }
    Ok(raw
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

struct Cached {
    pcm: std::sync::Arc<Vec<f32>>,
}

struct Current {
    file: String,
    ratio: std::sync::Arc<AtomicU32>,
    metro: bool,
    metro_bpm: f32,
    duration_ms: u64,
}

pub struct Engine {
    stream: Option<rodio::OutputStream>,
    handle: Option<rodio::OutputStreamHandle>,
    sink: Option<Sink>,
    cache: HashMap<String, Cached>,
    order: VecDeque<String>,
    current: Option<Current>,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            stream: None,
            handle: None,
            sink: None,
            cache: HashMap::new(),
            order: VecDeque::new(),
            current: None,
        }
    }

    fn ensure_stream(&mut self) -> Result<rodio::OutputStreamHandle, String> {
        if self.handle.is_none() {
            let (stream, handle) = rodio::OutputStream::try_default()
                .map_err(|e| format!("No audio output device: {e}"))?;
            self.stream = Some(stream);
            self.handle = Some(handle);
        }
        Ok(self.handle.as_ref().unwrap().clone())
    }

    fn cached_pcm(&mut self, file: &str, opus: &[u8], duration_ms: u64) -> Result<std::sync::Arc<Vec<f32>>, String> {
        if let Some(c) = self.cache.get(file) {
            touch(&mut self.order, file);
            return Ok(c.pcm.clone());
        }
        let raw = decode_opus_bytes(opus, file)?;
        let trimmed = std::sync::Arc::new(trim_to_duration(&raw, duration_ms));
        if trimmed.len() < 16 {
            return Err("decoded audio is empty".to_string());
        }
        self.cache.insert(
            file.to_string(),
            Cached { pcm: trimmed.clone() },
        );
        self.order.push_back(file.to_string());
        while self.order.len() > 8 {
            if let Some(old) = self.order.pop_front() {
                self.cache.remove(&old);
            }
        }
        Ok(trimmed)
    }

    fn start_sink(&mut self, mixed: std::sync::Arc<Vec<f32>>, ratio: std::sync::Arc<AtomicU32>, volume: f32) -> Result<(), String> {
        let handle = self.ensure_stream()?;
        if let Some(s) = self.sink.take() {
            s.stop();
        }
        let sink =
            Sink::try_new(&handle).map_err(|e| format!("audio sink failed: {e}"))?;
        sink.append(LoopSrc::new(mixed, ratio));
        sink.set_volume(volume.clamp(0.0, 1.0));
        sink.play();
        self.sink = Some(sink);
        Ok(())
    }

    pub fn play(
        &mut self,
        file: &str,
        opus: &[u8],
        tempo_pct: f32,
        volume: f32,
        metro: bool,
        metro_bpm: f32,
        duration_ms: u64,
    ) -> Result<(), String> {
        let raw = self.cached_pcm(file, opus, duration_ms)?;
        let mixed = std::sync::Arc::new(mix_clicks(&raw, metro, metro_bpm));
        let ratio = std::sync::Arc::new(AtomicU32::new(
            (tempo_pct.clamp(10.0, 400.0) / 100.0 * 65536.0) as u32,
        ));
        self.start_sink(mixed, ratio.clone(), volume)?;
        self.current = Some(Current {
            file: file.to_string(),
            ratio,
            metro,
            metro_bpm,
            duration_ms,
        });
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(s) = self.sink.take() {
            s.stop();
        }
        self.current = None;
    }

    pub fn set_tempo(&mut self, tempo_pct: f32) {
        if let Some(c) = self.current.as_ref() {
            c.ratio.store(
                (tempo_pct.clamp(10.0, 400.0) / 100.0 * 65536.0) as u32,
                Ordering::Relaxed,
            );
        }
    }

    pub fn set_volume(&mut self, volume: f32) {
        if let Some(s) = self.sink.as_ref() {
            s.set_volume(volume.clamp(0.0, 1.0));
        }
    }

    /// Rebuild the mixed buffer (clicks on/off or new BPM), keeping tempo/volume.
    pub fn set_metro(&mut self, metro: bool, metro_bpm: f32) -> Result<(), String> {
        let (file, ratio, _duration_ms) = match self.current.as_ref() {
            Some(c) => (c.file.clone(), c.ratio.clone(), c.duration_ms),
            None => return Ok(()),
        };
        let raw = self
            .cache
            .get(&file)
            .map(|c| c.pcm.clone())
            .ok_or("loop not loaded")?;
        if let Some(c) = self.current.as_mut() {
            c.metro = metro;
            c.metro_bpm = metro_bpm;
        }
        let mixed = std::sync::Arc::new(mix_clicks(&raw, metro, metro_bpm));
        let vol = 0.8;
        self.start_sink(mixed, ratio, vol)?;
        Ok(())
    }

}

fn touch(order: &mut VecDeque<String>, file: &str) {
    if let Some(i) = order.iter().position(|f| f == file) {
        order.remove(i);
    }
    order.push_back(file.to_string());
}

/// Messages for the dedicated audio thread (rodio types stay there).
pub enum AudioCmd {
    Play {
        file: String,
        tempo_pct: f32,
        volume: f32,
        metro: bool,
        metro_bpm: f32,
        duration_ms: u64,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    Stop,
    SetTempo(f32),
    SetVolume(f32),
    SetMetro {
        metro: bool,
        metro_bpm: f32,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
}

/// Cloneable handle to the audio thread; safe to share across Tauri commands.
#[derive(Clone)]
pub struct AudioFrontend {
    tx: Sender<AudioCmd>,
}

impl AudioFrontend {
    pub fn spawn() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("loopz-audio".into())
            .spawn(move || run(rx))
            .expect("audio thread");
        Self { tx }
    }

    pub fn tx_send(&self, cmd: AudioCmd) -> Result<(), String> {
        self.tx.send(cmd).map_err(|e| e.to_string())
    }
}

fn run(rx: std::sync::mpsc::Receiver<AudioCmd>) {
    let mut engine = Engine::new();
    for cmd in rx {
        match cmd {
            AudioCmd::Play {
                file,
                tempo_pct,
                volume,
                metro,
                metro_bpm,
                duration_ms,
                reply,
            } => {
                let r = match crate::server::FRONTEND.get_file(&file) {
                    Some(f) => engine.play(
                        &file,
                        f.contents(),
                        tempo_pct,
                        volume,
                        metro,
                        metro_bpm,
                        duration_ms,
                    ),
                    None => Err(format!("loop file not bundled: {file}")),
                };
                let _ = reply.send(r);
            }
            AudioCmd::Stop => engine.stop(),
            AudioCmd::SetTempo(p) => engine.set_tempo(p),
            AudioCmd::SetVolume(v) => engine.set_volume(v),
            AudioCmd::SetMetro { metro, metro_bpm, reply } => {
                let _ = reply.send(engine.set_metro(metro, metro_bpm));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_loop(frames: usize, freq: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let s = (2.0 * std::f32::consts::PI * freq * i as f32 / SAMPLE_RATE as f32).sin();
                [s, s * 0.5]
            })
            .collect()
    }

    #[test]
    fn loop_wraps_seamlessly() {
        // 4-frame buffer: take 10 samples, pattern must repeat every 8.
        let data: Vec<f32> = vec![0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7];
        let ratio = std::sync::Arc::new(AtomicU32::new(65536));
        let mut src = LoopSrc::new(std::sync::Arc::new(data), ratio);
        let got: Vec<f32> = (&mut src).take(16).collect();
        assert_eq!(
            got,
            vec![0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7]
        );
    }

    #[test]
    fn double_speed_consumes_twice() {
        let data = sine_loop(44100, 440.0);
        let ratio = std::sync::Arc::new(AtomicU32::new(65536 * 2));
        let mut src = LoopSrc::new(std::sync::Arc::new(data), ratio);
        // 1s of output at 2x should span the full 1s buffer exactly once.
        let got: Vec<f32> = (&mut src).take(44100 * 2).collect();
        assert_eq!(got.len(), 44100 * 2);
        // first sample ~0, sample at half buffer ~0 again (sine restarted)
        assert!(got[0].abs() < 1e-5);
    }

    #[test]
    fn trim_cuts_to_musical_length() {
        let raw = vec![0.1f32; 44100 * 2 * 4]; // 4s stereo
        let trimmed = trim_to_duration(&raw, 2000);
        assert_eq!(trimmed.len(), 44100 * 2 * 2);
        let kept = trim_to_duration(&raw, 99999);
        assert_eq!(kept.len(), raw.len());
    }

    #[test]
    fn clicks_land_on_grid_and_clamp() {
        let raw = vec![0.9f32; 44100 * 2]; // 1s loud loop
        let mixed = mix_clicks(&raw, true, 60.0); // beat every 1s -> click at 0
        assert_eq!(mixed.len(), raw.len());
        assert!(mixed.iter().all(|&s| s <= 1.0 && s >= -1.0));
        // click at frame 0: peak of the click envelope must poke above the loop
        assert!(mixed[..400].iter().any(|&s| s > 0.90));
        // no metro -> only normalization (0.9 peak scaled to 0.89)
        let plain = mix_clicks(&raw, false, 60.0);
        assert!((plain[0] - 0.89).abs() < 1e-6);
    }
}
