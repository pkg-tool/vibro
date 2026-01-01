use crate::stdout_is_a_pty;
use backtrace::{self, Backtrace};
use chrono::Utc;
use release_channel::{AppCommitSha, RELEASE_CHANNEL, ReleaseChannel};
use std::{
    env,
    ffi::c_void,
    sync::atomic::Ordering,
};
use std::{io::Write, panic, sync::atomic::AtomicU32, thread};
use util::ResultExt;

static PANIC_COUNT: AtomicU32 = AtomicU32::new(0);

mod system_info {
    pub fn os_name() -> String {
        #[cfg(target_os = "macos")]
        {
            "macOS".to_string()
        }

        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            format!("Linux {}", gpui::guess_compositor())
        }

        #[cfg(target_os = "windows")]
        {
            "Windows".to_string()
        }
    }

    pub fn os_version() -> String {
        #[cfg(target_os = "macos")]
        {
            let output = std::process::Command::new("/usr/bin/sw_vers")
                .args(["-productVersion"])
                .output();
            match output {
                Ok(output) if output.status.success() => {
                    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if version.is_empty() {
                        "unknown".to_string()
                    } else {
                        version
                    }
                }
                _ => "unknown".to_string(),
            }
        }

        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            use std::path::Path;

            let content = if let Ok(file) = std::fs::read_to_string(&Path::new("/etc/os-release"))
            {
                file
            } else if let Ok(file) = std::fs::read_to_string(&Path::new("/usr/lib/os-release"))
            {
                file
            } else {
                log::error!("Failed to load /etc/os-release, /usr/lib/os-release");
                "".to_string()
            };
            let mut name = "unknown";
            let mut version = "unknown";

            for line in content.lines() {
                match line.split_once('=') {
                    Some(("ID", val)) => name = val.trim_matches('"'),
                    Some(("VERSION_ID", val)) => version = val.trim_matches('"'),
                    _ => {}
                }
            }

            format!("{name} {version}")
        }

        #[cfg(target_os = "windows")]
        {
            let mut info = unsafe { std::mem::zeroed() };
            let status = unsafe { windows::Wdk::System::SystemServices::RtlGetVersion(&mut info) };
            if status.is_ok() {
                format!(
                    "{}.{}.{}",
                    info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber
                )
            } else {
                "unknown".to_string()
            }
        }
    }
}

#[derive(serde::Serialize)]
struct LocationData {
    file: String,
    line: u32,
}

#[derive(serde::Serialize)]
struct PanicReport {
    thread: String,
    payload: String,
    location_data: Option<LocationData>,
    app_version: String,
    app_commit_sha: Option<String>,
    release_channel: String,
    target: String,
    os_name: String,
    os_version: Option<String>,
    architecture: String,
    panicked_on: i64,
    backtrace: Vec<String>,
}

pub fn init_panic_hook(
    app_version: String,
    app_commit_sha: Option<AppCommitSha>,
) {
    let is_pty = stdout_is_a_pty();

    panic::set_hook(Box::new(move |info| {
        let prior_panic_count = PANIC_COUNT.fetch_add(1, Ordering::SeqCst);
        if prior_panic_count > 0 {
            // Give the panic-ing thread time to write the panic file
            loop {
                std::thread::yield_now();
            }
        }

        let thread = thread::current();
        let thread_name = thread.name().unwrap_or("<unnamed>");

        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "Box<Any>".to_string());

        if *release_channel::RELEASE_CHANNEL == ReleaseChannel::Dev {
            let location = info.location().unwrap();
            let backtrace = Backtrace::new();
            eprintln!(
                "Thread {:?} panicked with {:?} at {}:{}:{}\n{}{:?}",
                thread_name,
                payload,
                location.file(),
                location.line(),
                location.column(),
                match app_commit_sha.as_ref() {
                    Some(commit_sha) => format!("commit {}\n", commit_sha.full()),
                    None => "".to_string(),
                },
                backtrace,
            );
            std::process::exit(-1);
        }
        let main_module_base_address = get_main_module_base_address();

        let backtrace = Backtrace::new();
        let mut symbols = backtrace
            .frames()
            .iter()
            .flat_map(|frame| {
                let base = frame
                    .module_base_address()
                    .unwrap_or(main_module_base_address);
                frame.symbols().iter().map(move |symbol| {
                    format!(
                        "{}+{}",
                        symbol
                            .name()
                            .as_ref()
                            .map_or("<unknown>".to_owned(), <_>::to_string),
                        (frame.ip() as isize).saturating_sub(base as isize)
                    )
                })
            })
            .collect::<Vec<_>>();

        // Strip out leading stack frames for rust panic-handling.
        if let Some(ix) = symbols
            .iter()
            .position(|name| name == "rust_begin_unwind" || name == "_rust_begin_unwind")
        {
            symbols.drain(0..=ix);
        }

        let panic_data = PanicReport {
            thread: thread_name.into(),
            payload,
            location_data: info.location().map(|location| LocationData {
                file: location.file().into(),
                line: location.line(),
            }),
            app_version: app_version.clone(),
            app_commit_sha: app_commit_sha.as_ref().map(|sha| sha.full()),
            release_channel: RELEASE_CHANNEL.dev_name().into(),
            target: env!("TARGET").to_owned(),
            os_name: system_info::os_name(),
            os_version: Some(system_info::os_version()),
            architecture: env::consts::ARCH.into(),
            panicked_on: Utc::now().timestamp_millis(),
            backtrace: symbols,
        };

        if let Some(panic_data_json) = serde_json::to_string_pretty(&panic_data).log_err() {
            log::error!("{}", panic_data_json);
        }
        zlog::flush();

        if !is_pty {
            if let Some(panic_data_json) = serde_json::to_string(&panic_data).log_err() {
                let timestamp = chrono::Utc::now().format("%Y_%m_%d %H_%M_%S").to_string();
                let panic_file_path = paths::logs_dir().join(format!("vector-{timestamp}.panic"));
                let panic_file = std::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(&panic_file_path)
                    .log_err();
                if let Some(mut panic_file) = panic_file {
                    writeln!(&mut panic_file, "{panic_data_json}").log_err();
                    panic_file.flush().log_err();
                }
            }
        }

        std::process::abort();
    }));
}

#[cfg(not(target_os = "windows"))]
fn get_main_module_base_address() -> *mut c_void {
    let mut dl_info = libc::Dl_info {
        dli_fname: std::ptr::null(),
        dli_fbase: std::ptr::null_mut(),
        dli_sname: std::ptr::null(),
        dli_saddr: std::ptr::null_mut(),
    };
    unsafe {
        libc::dladdr(get_main_module_base_address as _, &mut dl_info);
    }
    dl_info.dli_fbase
}

#[cfg(target_os = "windows")]
fn get_main_module_base_address() -> *mut c_void {
    std::ptr::null_mut()
}
