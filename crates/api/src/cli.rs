//! Kelola akun dari terminal Pi (tanpa SQL manual):
//!
//!   bin/api user add <username> [owner|tenant]   buat akun (password ditanya, tidak tampil)
//!   bin/api user passwd <username>               ganti password + cabut semua sesinya
//!   bin/api user list                            daftar akun + jumlah sesi aktif
//!   bin/api user del <username>                  hapus akun + sesinya
//!   bin/api user logout <username>               cabut semua sesi (paksa login ulang)
//!   bin/api user log [jumlah]                    catatan login terakhir (default 30)
//!
//! Jalankan dari WorkingDirectory service dengan .env yang sama (SQLITE_PATH).

use std::io::{BufRead, IsTerminal, Write};

use sqlx::SqlitePool;

use crate::authcore::check_password_policy;

fn read_secret(prompt: &str) -> anyhow::Result<String> {
    print!("{prompt}");
    std::io::stdout().flush()?;
    let tty = std::io::stdin().is_terminal();
    if tty {
        let _ = std::process::Command::new("stty").arg("-echo").status();
    }
    let mut line = String::new();
    let r = std::io::stdin().lock().read_line(&mut line);
    if tty {
        let _ = std::process::Command::new("stty").arg("echo").status();
        println!();
    }
    r?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

fn ask_new_password(username: &str) -> anyhow::Result<String> {
    loop {
        let p1 = read_secret("Password baru: ")?;
        if let Err(msg) = check_password_policy(username, &p1) {
            eprintln!("Ditolak: {msg}.");
            if !std::io::stdin().is_terminal() {
                anyhow::bail!("password ditolak");
            }
            continue;
        }
        let p2 = read_secret("Ulangi password: ")?;
        if p1 != p2 {
            eprintln!("Password tidak sama, ulangi.");
            if !std::io::stdin().is_terminal() {
                anyhow::bail!("password tidak sama");
            }
            continue;
        }
        return Ok(p1);
    }
}

fn valid_username(u: &str) -> bool {
    (3..=32).contains(&u.len()) && u.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

pub async fn run(args: &[String], db: &SqlitePool) -> anyhow::Result<()> {
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let name = args.get(1).map(String::as_str).unwrap_or("");
    match cmd {
        "add" => {
            if !valid_username(name) {
                anyhow::bail!("username 3-32 karakter: huruf, angka, _ - .");
            }
            let role = args.get(2).map(String::as_str).unwrap_or("owner");
            if role != "owner" && role != "tenant" {
                anyhow::bail!("peran harus owner atau tenant");
            }
            let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE username = ?").bind(name).fetch_optional(db).await?;
            if exists.is_some() {
                anyhow::bail!("akun {name} sudah ada (pakai: user passwd {name})");
            }
            let pw = ask_new_password(name)?;
            let hash = bcrypt::hash(pw, 12)?;
            sqlx::query("INSERT INTO users (username, password_hash, role) VALUES (?, ?, ?)")
                .bind(name)
                .bind(hash)
                .bind(role)
                .execute(db)
                .await?;
            println!("Akun {name} ({role}) dibuat.");
        }
        "passwd" => {
            let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE username = ?").bind(name).fetch_optional(db).await?;
            if exists.is_none() {
                anyhow::bail!("akun {name} tidak ada");
            }
            let pw = ask_new_password(name)?;
            let hash = bcrypt::hash(pw, 12)?;
            sqlx::query("UPDATE users SET password_hash = ? WHERE username = ?").bind(hash).bind(name).execute(db).await?;
            let n = sqlx::query("DELETE FROM sessions WHERE username = ?").bind(name).execute(db).await?.rows_affected();
            println!("Password {name} diganti, {n} sesi dicabut.");
        }
        "list" => {
            let rows: Vec<(String, String, String, i64)> = sqlx::query_as(
                "SELECT u.username, u.role, u.created_at, (SELECT COUNT(*) FROM sessions s WHERE s.username = u.username) \
                 FROM users u ORDER BY u.username",
            )
            .fetch_all(db)
            .await?;
            if rows.is_empty() {
                println!("Belum ada akun. Buat: bin/api user add admin owner");
            }
            for (u, r, c, n) in rows {
                println!("{u:<20} {r:<8} dibuat {c}  sesi aktif: {n}");
            }
        }
        "del" => {
            let n = sqlx::query("DELETE FROM users WHERE username = ?").bind(name).execute(db).await?.rows_affected();
            sqlx::query("DELETE FROM sessions WHERE username = ?").bind(name).execute(db).await?;
            println!("{}", if n > 0 { format!("Akun {name} dihapus.") } else { format!("Akun {name} tidak ada.") });
        }
        "logout" => {
            let n = sqlx::query("DELETE FROM sessions WHERE username = ?").bind(name).execute(db).await?.rows_affected();
            println!("{n} sesi {name} dicabut.");
        }
        "log" => {
            let limit: i64 = name.parse().unwrap_or(30);
            let rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
                "SELECT datetime(ts, 'unixepoch', 'localtime'), username, ip, event, detail FROM auth_events ORDER BY ts DESC LIMIT ?",
            )
            .bind(limit)
            .fetch_all(db)
            .await?;
            for (t, u, ip, e, d) in rows {
                println!("{t}  {e:<16} {u:<16} {ip:<16} {d}");
            }
        }
        _ => {
            println!(
                "Pemakaian: api user <add|passwd|list|del|logout|log> ...\n  \
                 add <username> [owner|tenant] | passwd <username> | list | del <username> | logout <username> | log [n]"
            );
        }
    }
    Ok(())
}
