#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod favs;
mod server;

use std::sync::Mutex;
use tauri::{Manager, State};

use favs::FavStore;

struct PhoneServer(Mutex<Option<server::RunningServer>>);

use audio::{AudioCmd, AudioFrontend};

/// Start the LAN server (idempotent: returns the existing URL if already on).
#[tauri::command]
async fn start_phone_server(
    state: State<'_, PhoneServer>,
    favs: State<'_, FavStore>,
) -> Result<String, String> {
    {
        let guard = state.0.lock().map_err(|e| e.to_string())?;
        if let Some(running) = guard.as_ref() {
            return Ok(running.url.clone());
        }
    }
    let running = server::start(favs.inner().clone()).await?;
    let url = running.url.clone();
    *state.0.lock().map_err(|e| e.to_string())? = Some(running);
    Ok(url)
}

/// Print a frontend message to the terminal (for audio debugging).
#[tauri::command]
fn debug_log(msg: String) {
    eprintln!("[web] {msg}");
}

/// Start/resume loop playback through the native Rust audio engine.
#[tauri::command]
async fn audio_play(
    state: State<'_, AudioFrontend>,
    file: String,
    tempo_pct: f32,
    volume: f32,
    metro: bool,
    metro_bpm: f32,
    duration_ms: u64,
) -> Result<(), String> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    state
        .tx_send(AudioCmd::Play {
            file,
            tempo_pct,
            volume,
            metro,
            metro_bpm,
            duration_ms,
            reply: reply_tx,
        })
        .map_err(|e| format!("audio engine gone: {e}"))?;
    reply_rx.await.map_err(|e| format!("audio engine gone: {e}"))?
}

#[tauri::command]
async fn audio_stop(state: State<'_, AudioFrontend>) -> Result<(), String> {
    state
        .tx_send(AudioCmd::Stop)
        .map_err(|e| format!("audio engine gone: {e}"))?;
    Ok(())
}

#[tauri::command]
async fn audio_set_tempo(state: State<'_, AudioFrontend>, tempo_pct: f32) -> Result<(), String> {
    state
        .tx_send(AudioCmd::SetTempo(tempo_pct))
        .map_err(|e| format!("audio engine gone: {e}"))?;
    Ok(())
}

#[tauri::command]
async fn audio_set_volume(state: State<'_, AudioFrontend>, volume: f32) -> Result<(), String> {
    state
        .tx_send(AudioCmd::SetVolume(volume))
        .map_err(|e| format!("audio engine gone: {e}"))?;
    Ok(())
}

#[tauri::command]
async fn audio_set_metro(
    state: State<'_, AudioFrontend>,
    metro: bool,
    metro_bpm: f32,
) -> Result<(), String> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    state
        .tx_send(AudioCmd::SetMetro {
            metro,
            metro_bpm,
            reply: reply_tx,
        })
        .map_err(|e| format!("audio engine gone: {e}"))?;
    reply_rx.await.map_err(|e| format!("audio engine gone: {e}"))?
}

#[tauri::command]
async fn stop_phone_server(state: State<'_, PhoneServer>) -> Result<(), String> {
    let running = state.0.lock().map_err(|e| e.to_string())?.take();
    if let Some(running) = running {
        running.shutdown().await;
    }
    Ok(())
}

/// Shared favorites (desktop + phone read/write the same on-disk set).
#[tauri::command]
fn fav_list(store: State<'_, FavStore>) -> Vec<String> {
    store.list()
}

/// Toggle one favorite, persist, return the full updated list.
#[tauri::command]
fn fav_set(store: State<'_, FavStore>, file: String, fav: bool) -> Vec<String> {
    store.set(file, fav)
}

fn main() {
    tauri::Builder::default()
        .manage(PhoneServer(Mutex::new(None)))
        .manage(audio::AudioFrontend::spawn())
        .setup(|app| {
            let dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("loopz"));
            app.manage(FavStore::load(dir));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_phone_server,
            stop_phone_server,
            debug_log,
            fav_list,
            fav_set,
            audio_play,
            audio_stop,
            audio_set_tempo,
            audio_set_volume,
            audio_set_metro
        ])
        .run(tauri::generate_context!())
        .expect("error while running Loopz");
}
