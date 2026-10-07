<#
.SYNOPSIS
    Build semua komponen (Rust binary via cross-compile + frontend WASM) di Windows,
    lalu kirim hasilnya ke Raspberry Pi 4B via scp. Raspi TIDAK compile apa pun.

.PARAMETER Remote
    Alamat SSH tujuan, format: user@ip-raspi (contoh: pi@100.96.160.34)

.PARAMETER RemotePath
    Folder tujuan di Raspi (default: /home/supriadi/Dev/smartkost)

.EXAMPLE
    .\deploy.ps1 -Remote pi@100.96.160.34 -RemotePath /home/supriadi/Dev/smartkost

.NOTES
    Prasyarat di Windows (native, tanpa WSL):
      - Docker Desktop terinstal & running (dipakai `cross` untuk toolchain ARM)
      - Rust: winget install Rustlang.Rustup
      - cargo install cross --git https://github.com/cross-rs/cross
      - rustup target add wasm32-unknown-unknown
      - cargo install trunk
      - OpenSSH Client (sudah bawaan Windows 10/11, cek: Get-WindowsCapability -Online -Name OpenSSH.Client*)
#>

param(
    [Parameter(Mandatory = $true)]
    [string]$Remote,

    [string]$RemotePath = "/home/supriadi/Dev/smartkost"
)

$ErrorActionPreference = "Stop"
$Target = "aarch64-unknown-linux-gnu"

function Step($msg) {
    Write-Host ""
    Write-Host "=== $msg ===" -ForegroundColor Cyan
}

# --- 1. Cross-compile binary Rust ---
Step "1/5: Cross-compile binary Rust (ingest, api, alertengine, vision, doorbell) untuk $Target"
cross build --release --target $Target -p ingest
if ($LASTEXITCODE -ne 0) { throw "cross build gagal untuk ingest" }

cross build --release --target $Target -p api
if ($LASTEXITCODE -ne 0) { throw "cross build gagal untuk api" }

cross build --release --target $Target -p alertengine
if ($LASTEXITCODE -ne 0) { throw "cross build gagal untuk alertengine" }

cross build --release --target $Target -p vision
if ($LASTEXITCODE -ne 0) { throw "cross build gagal untuk vision" }

cross build --release --target $Target -p doorbell
if ($LASTEXITCODE -ne 0) { throw "cross build gagal untuk doorbell" }

# --- 2. Build frontend ---
Step "2/5: Build frontend (Leptos -> WASM)"
Push-Location frontend-leptos
try {
    trunk build --release
    if ($LASTEXITCODE -ne 0) { throw "trunk build gagal" }
} finally {
    Pop-Location
}

# --- 3. Siapkan folder remote di Raspi ---
Step "3/5: Siapkan folder remote di Raspi ($Remote`:$RemotePath)"
ssh $Remote "mkdir -p $RemotePath/bin $RemotePath/frontend $RemotePath/data $RemotePath/deploy/go2rtc $RemotePath/config $RemotePath/models"
if ($LASTEXITCODE -ne 0) { throw "gagal ssh/mkdir di remote. Cek koneksi & kredensial SSH." }

# --- 4. Transfer file ke Raspi ---
Step "4/5: Transfer file ke Raspi"

$binPath = "target/$Target/release"
scp "$binPath/ingest" "${Remote}:${RemotePath}/bin/"
scp "$binPath/api" "${Remote}:${RemotePath}/bin/"
scp "$binPath/alertengine" "${Remote}:${RemotePath}/bin/"
scp "$binPath/vision" "${Remote}:${RemotePath}/bin/"
scp "$binPath/doorbell" "${Remote}:${RemotePath}/bin/"
scp -r "frontend-leptos/dist/." "${Remote}:${RemotePath}/frontend/"
scp "deploy/go2rtc/go2rtc.yaml" "${Remote}:${RemotePath}/deploy/go2rtc/"

# go2rtc binary: ambil release terbaru untuk arm64 dari GitHub
$go2rtcRemote = "${Remote}:${RemotePath}/bin/go2rtc"
$go2rtcUrl = "https://github.com/AlexxIT/go2rtc/releases/latest/download/go2rtc_linux_arm64"
Invoke-WebRequest -Uri $go2rtcUrl -OutFile "$env:TEMP\go2rtc_linux_arm64"
scp "$env:TEMP\go2rtc_linux_arm64" $go2rtcRemote
ssh $Remote "chmod +x $RemotePath/bin/go2rtc"

# config kamera / .env
$camConfigCheck = ssh $Remote "[ -f $RemotePath/config/cameras.toml ] && echo EXISTS || echo MISSING"
if ($camConfigCheck -match "EXISTS") {
    Write-Host "config/cameras.toml sudah ada di Raspi, tidak ditimpa." -ForegroundColor Yellow
} else {
    scp "config/cameras.toml" "${Remote}:${RemotePath}/config/"
    Write-Host "PENTING: edit $RemotePath/config/cameras.toml sebelum start smartkost-vision!" -ForegroundColor Yellow
}

$nvrConfigCheck = ssh $Remote "[ -f $RemotePath/config/nvr.toml ] && echo EXISTS || echo MISSING"
if ($nvrConfigCheck -match "EXISTS") {
    Write-Host "config/nvr.toml sudah ada di Raspi, tidak ditimpa." -ForegroundColor Yellow
} else {
    scp "config/nvr.toml" "${Remote}:${RemotePath}/config/"
    Write-Host "PENTING: edit $RemotePath/config/nvr.toml sebelum start smartkost-api!" -ForegroundColor Yellow
}

$doorCheck = ssh $Remote "[ -f $RemotePath/config/doorbell.toml ] && echo EXISTS || echo MISSING"
if ($doorCheck -match "EXISTS") {
    Write-Host "config/doorbell.toml sudah ada di Raspi, tidak ditimpa." -ForegroundColor Yellow
} else {
    scp "config/doorbell.toml" "${Remote}:${RemotePath}/config/"
    Write-Host "PENTING: cek $RemotePath/config/doorbell.toml (door_id, teks, zone.roi) sebelum start smartkost-doorbell!" -ForegroundColor Yellow
}

$envCheck = ssh $Remote "[ -f $RemotePath/.env ] && echo EXISTS || echo MISSING"
if ($envCheck -match "EXISTS") {
    Write-Host "File .env sudah ada di Raspi, tidak ditimpa." -ForegroundColor Yellow
} else {
    Write-Host "Belum ada .env di Raspi, mengirim .env.example sebagai referensi awal..." -ForegroundColor Yellow
    scp ".env.example" "${Remote}:${RemotePath}/.env"
    Write-Host "PENTING: edit $RemotePath/.env di Raspi sebelum menjalankan service!" -ForegroundColor Yellow
}

ssh $Remote "chmod +x $RemotePath/bin/*"

# --- 5. Copy service unit ke remote (opsional, but recommended) ---
Step "5/5: Menyiapkan file systemd untuk semua service"
$serviceFiles = @(
    "deploy/systemd/smartkost-ingest.service",
    "deploy/systemd/smartkost-api.service",
    "deploy/systemd/smartkost-alertengine.service",
    "deploy/systemd/smartkost-vision.service",
    "deploy/systemd/smartkost-go2rtc.service",
    "deploy/systemd/smartkost-doorbell.service"
)
foreach ($service in $serviceFiles) {
    if (Test-Path $service) {
        scp $service "${Remote}:/tmp/"
        ssh $Remote "sudo mv /tmp/$(Split-Path -Leaf $service) /etc/systemd/system/"
    }
}

ssh $Remote "sudo systemctl daemon-reload"

Write-Host ""
Write-Host "=== Selesai ===" -ForegroundColor Green
Write-Host "Binary, frontend, config, dan go2rtc sudah ada di ${Remote}:${RemotePath}"
Write-Host "Langkah selanjutnya (di Raspi):"
Write-Host "  1. Edit $RemotePath/.env (isi token, secret, dsb)"
Write-Host "  2. Edit $RemotePath/config/cameras.toml jika perlu"
Write-Host "  3. sudo systemctl enable --now smartkost-go2rtc"
Write-Host "  (doorbell) tambahkan DOORBELL_CAMERA_TOKEN ke .env dan cek config/doorbell.toml - lihat docs/DOORBELL.md"
Write-Host "  4. sudo systemctl enable --now smartkost-ingest smartkost-api smartkost-alertengine smartkost-vision smartkost-doorbell"