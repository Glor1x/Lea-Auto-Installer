#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{BufRead, BufReader, Read};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tauri::{AppHandle, Emitter, Listener, Manager, Window};
use tauri_plugin_dialog::DialogExt;
use tokio::fs;
use tokio::io::AsyncWriteExt;

const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Clone, serde::Serialize)]
struct ProgressPayload {
    percent: u32,
    status: String,
    step: String,
}

#[derive(serde::Serialize)]
struct FolderResult {
    path: String,
    is_valid: bool,
}

#[derive(serde::Serialize)]
struct InstallResult {
    success: bool,
    error: Option<String>,
}

fn is_admin() -> bool {
    let output = Command::new("net")
        .args(["session"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    match output {
        Ok(o) => o.status.success(),
        Err(_) => false,
    }
}

fn relaunch_as_admin() {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let exe = std::env::current_exe().unwrap();

    fn to_wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }

    let operation = to_wide("runas");
    let file     = to_wide(&exe.to_string_lossy());
    let params   = to_wide("--elevated");

    unsafe {
        ShellExecuteW(
            HWND(std::ptr::null_mut()),
            PCWSTR(operation.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR(params.as_ptr()),
            PCWSTR(std::ptr::null()),
            SW_HIDE,
        );
    }

    std::process::exit(0);
}

#[tauri::command]
fn check_game_running() -> bool {
    let output = Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq gta_sa.exe", "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            stdout.to_lowercase().contains("gta_sa.exe")
        }
        Err(_) => false,
    }
}

#[tauri::command]
fn kill_game() -> bool {
    let output = Command::new("taskkill")
        .args(["/F", "/IM", "gta_sa.exe"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    match output {
        Ok(o) => o.status.success(),
        Err(_) => false,
    }
}

#[tauri::command]
async fn select_folder(app: AppHandle) -> Result<Option<FolderResult>, String> {
    let (tx, rx) = std::sync::mpsc::channel();

    app.dialog()
        .file()
        .set_title("Выберите папку с GTA San Andreas")
        .pick_folder(move |path| {
            let _ = tx.send(path);
        });

    let path = rx.recv().map_err(|e| e.to_string())?;

    match path {
        Some(p) => {
            let path_buf = p.as_path()
                .ok_or("Неверный путь".to_string())?
                .to_path_buf();
            let is_valid = check_game_path(&path_buf).await;
            Ok(Some(FolderResult {
                path: path_buf.to_string_lossy().to_string(),
                is_valid,
            }))
        }
        None => Ok(None),
    }
}

#[tauri::command]
async fn validate_path(path: String) -> FolderResult {
    let path_buf = PathBuf::from(&path);
    let is_valid = check_game_path(&path_buf).await;
    FolderResult { path, is_valid }
}

async fn check_game_path(path: &PathBuf) -> bool {
    fs::metadata(path.join("gta_sa.exe")).await.is_ok()
}

#[tauri::command]
fn minimize_app(window: Window) {
    let _ = window.minimize();
}

#[tauri::command]
fn close_app(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
async fn start_installation(
    window: Window,
    game_path: String,
) -> Result<InstallResult, String> {
    match run_installation(window.clone(), game_path).await {
        Ok(_) => Ok(InstallResult { success: true, error: None }),
        Err(e) => {
            let msg = e.to_string();
            let _ = window.emit("install-progress", ProgressPayload {
                percent: 0,
                status: format!("Ошибка: {}", msg),
                step: "error".to_string(),
            });
            Ok(InstallResult { success: false, error: Some(msg) })
        }
    }
}

fn emit_progress(window: &Window, percent: u32, status: &str, step: &str) {
    let _ = window.emit("install-progress", ProgressPayload {
        percent,
        status: status.to_string(),
        step: step.to_string(),
    });
}

async fn run_installation(window: Window, game_path: String) -> anyhow::Result<()> {
    let archive_url = "https://lea-script.space/system/LEA_setup_files.zip";
    let manager_url = "https://lea-script.space/system/scripts/Law%20Enforcer%20Assistant%20Manager.luac";

    let temp_dir = std::env::temp_dir().join("lea-installer");
    fs::create_dir_all(&temp_dir).await?;

    let archive_path = temp_dir.join("LEA_setup_files.zip");
    let game_path_buf = PathBuf::from(&game_path);

    emit_progress(&window, 0, "Начинаем скачивание LEA...", "download");

    download_file_with_progress(
        &window, archive_url, &archive_path, 0, 40, "download",
    ).await?;

    let meta = fs::metadata(&archive_path).await?;
    if meta.len() == 0 {
        anyhow::bail!("Скачанный файл пустой");
    }

    emit_progress(&window, 40, "Распаковываем архив...", "extract");

    let archive_path_clone = archive_path.clone();
    let game_path_clone = game_path_buf.clone();
    let window_clone = window.clone();
    tokio::task::spawn_blocking(move || {
        extract_with_powershell_progress(&window_clone, &archive_path_clone, &game_path_clone, 40, 80)
    }).await??;

    emit_progress(&window, 80, "Архив распакован!", "extract");

    let moonloader_dir = game_path_buf.join("moonloader");
    fs::create_dir_all(&moonloader_dir).await?;
    let manager_path = moonloader_dir.join("Law Enforcer Assistant Manager.luac");

    emit_progress(&window, 80, "Скачивание менеджера LEA...", "extract");

    download_file_with_progress(
        &window, manager_url, &manager_path, 80, 98, "extract",
    ).await?;

    let _ = fs::remove_file(&archive_path).await;

    emit_progress(&window, 100, "LEA успешно установлен!", "complete");

    Ok(())
}

async fn download_file_with_progress(
    window: &Window,
    url: &str,
    destination: &PathBuf,
    percent_start: u32,
    percent_end: u32,
    step: &str,
) -> anyhow::Result<()> {
    use futures_util::StreamExt;

    let client = reqwest::Client::new();
    let response = client.get(url).send().await?;

    if !response.status().is_success() {
        anyhow::bail!("HTTP ошибка: {}", response.status());
    }

    let total_size = response.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;
    let mut file = tokio::fs::File::create(destination).await?;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;

        let local_pct = if total_size > 0 {
            (downloaded as f64 / total_size as f64 * 100.0) as u32
        } else {
            50
        };

        let global_pct =
            percent_start + (local_pct * (percent_end - percent_start) / 100);

        let status = if step == "download" {
            format!("Скачивание архива: {}%", local_pct)
        } else {
            format!("Скачивание менеджера: {}%", local_pct)
        };

        emit_progress(window, global_pct.min(percent_end), &status, step);
    }

    file.flush().await?;
    file.sync_all().await?;

    let meta = fs::metadata(destination).await?;
    if meta.len() == 0 {
        anyhow::bail!("Скачанный файл пустой");
    }

    Ok(())
}

fn powershell_quote(path: &PathBuf) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "''"))
}

fn extract_with_powershell_progress(
    window: &Window,
    archive_path: &PathBuf,
    destination: &PathBuf,
    percent_start: u32,
    percent_end: u32,
) -> anyhow::Result<()> {
    let ps_command = format!(
        r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archivePath = {archive_path}
$destination = {destination}
$archive = [System.IO.Compression.ZipFile]::OpenRead($archivePath)
try {{
    $entries = @($archive.Entries | Where-Object {{ -not [string]::IsNullOrEmpty($_.Name) }})
    $totalBytes = [Int64]($entries | Measure-Object -Property Length -Sum).Sum
    if ($totalBytes -le 0) {{ $totalBytes = 1 }}
    $doneBytes = [Int64]0

    foreach ($entry in $entries) {{
        $targetPath = Join-Path $destination $entry.FullName
        $targetDir = Split-Path $targetPath -Parent
        if (-not [string]::IsNullOrEmpty($targetDir)) {{
            [System.IO.Directory]::CreateDirectory($targetDir) | Out-Null
        }}

        [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $targetPath, $true)
        $doneBytes += [Int64]$entry.Length
        $localPercent = [Math]::Min(100, [Math]::Floor(($doneBytes * 100) / $totalBytes))
        [Console]::Out.WriteLine("PROGRESS:$localPercent")
        [Console]::Out.Flush()
    }}
}} finally {{
    $archive.Dispose()
}}
"#,
        archive_path = powershell_quote(archive_path),
        destination = powershell_quote(destination),
    );

    let mut child = Command::new("powershell")
        .args([
            "-WindowStyle", "Hidden",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &ps_command,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("Не удалось прочитать прогресс распаковки"))?;

    for line in BufReader::new(stdout).lines() {
        let line = line?;
        if let Some(raw_percent) = line.strip_prefix("PROGRESS:") {
            if let Ok(local_percent) = raw_percent.trim().parse::<u32>() {
                let global_percent = percent_start
                    + (local_percent.min(100) * (percent_end - percent_start) / 100);
                emit_progress(
                    window,
                    global_percent.min(percent_end),
                    &format!("Распаковка архива: {}%", local_percent.min(100)),
                    "extract",
                );
            }
        }
    }

    let status = child.wait()?;
    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_string(&mut stderr);
        }
        anyhow::bail!("PowerShell ошибка: {}", stderr.trim());
    }

    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let already_elevated = args.contains(&"--elevated".to_string());

    #[cfg(not(debug_assertions))]
    if !already_elevated && !is_admin() {
        relaunch_as_admin();
        return;
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let window = app.get_webview_window("main").unwrap();
            let window_clone = window.clone();

            window.listen("page-ready", move |_| {
                window_clone.show().unwrap();
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            select_folder,
            validate_path,
            minimize_app,
            close_app,
            start_installation,
            check_game_running,
            kill_game,
        ])
        .run(tauri::generate_context!())
        .expect("Ошибка запуска Tauri");
}
