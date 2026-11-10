//! xtask — вспомогательные команды разработки.
//!
//! `cargo xtask contract --server <путь к checkout backspace>` поднимает
//! оригинальный сервер Backspace (Node.js, `pnpm install` уже выполнен) на
//! свободном порту с временной базой и прогоняет `bs-contract` против него.
//! Можно указать уже запущенный сервер: `cargo xtask contract --url <адрес>`.

#![forbid(unsafe_code)]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};

const USAGE: &str = "\
использование:
  cargo xtask contract --server <путь к backspace>   поднять оригинальный сервер и проверить клиент
  cargo xtask contract --url <http://host:port>      проверить клиент против запущенного сервера
  флаг --strict: падать и на полях, которые клиент не хранит";

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("xtask: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("contract") {
        return contract(&args[1..]);
    }
    println!("{USAGE}");
    Ok(ExitCode::SUCCESS)
}

/// Сервер, который останавливается при выходе из области видимости.
struct Server {
    child: Child,
    data: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.data);
    }
}

fn contract(args: &[String]) -> Result<ExitCode> {
    let mut server_dir = None;
    let mut url = None;
    let mut strict = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--server" => server_dir = iter.next().map(PathBuf::from),
            "--url" => url = iter.next().cloned(),
            "--strict" => strict = true,
            other => bail!("неизвестный аргумент {other}\n{USAGE}"),
        }
    }

    let _guard;
    let base = match (url, server_dir) {
        (Some(url), _) => url,
        (None, Some(dir)) => {
            let (server, base) = start_original(&dir)?;
            _guard = server;
            base
        }
        (None, None) => bail!("нужен --server или --url\n{USAGE}"),
    };

    println!("контрактные тесты против {base}");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let status = Command::new(cargo)
        .args([
            "test",
            "-p",
            "bs-contract",
            "--",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("BS_CONTRACT_URL", &base)
        .env("BS_CONTRACT_STRICT", if strict { "1" } else { "0" })
        .status()
        .context("не удалось запустить cargo test")?;
    Ok(if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Запускает `packages/server` оригинала на свободном порту.
fn start_original(checkout: &Path) -> Result<(Server, String)> {
    let server_pkg = checkout.join("packages/server");
    if !server_pkg.join("src/index.ts").is_file() {
        bail!("{} не похож на checkout backspace", checkout.display());
    }
    let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let data = std::env::temp_dir().join(format!("bs-contract-{port}"));
    std::fs::create_dir_all(data.join("uploads"))?;
    let secret: String = std::iter::repeat_n('k', 48).collect();
    let log = std::fs::File::create(data.join("server.log"))?;
    let child = Command::new("node")
        .args(["--import", "tsx/esm", "src/index.ts"])
        .current_dir(&server_pkg)
        .env("PORT", port.to_string())
        .env("HOST", "127.0.0.1")
        .env("JWT_SECRET", secret)
        .env("DB_PATH", data.join("backspace.db"))
        .env("UPLOAD_DIR", data.join("uploads"))
        .env("REGISTRATION_OPEN", "true")
        .env("DIRECTORY_ENDPOINT", "")
        .env("NODE_ENV", "development")
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .spawn()
        .context("не удалось запустить node (нужен Node.js 22+ и pnpm install)")?;
    let server = Server {
        child,
        data: data.clone(),
    };
    let base = format!("http://127.0.0.1:{port}");
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            // Порт открыт; даём серверу закончить миграции.
            std::thread::sleep(Duration::from_millis(500));
            return Ok((server, base));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    bail!(
        "сервер не поднялся за 60 с, журнал: {}",
        data.join("server.log").display()
    )
}
