use std::{
    ffi::OsString,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
    collections::HashSet,
};
use chrono::{Local, NaiveDate, NaiveTime};
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};
use crate::config::Config;
use crate::ipc::{Command as IpcCommand, PIPE_NAME};
use std::io::Write;
use std::fs::OpenOptions;

const SERVICE_NAME: &str = "TheWorldIsGreyShutItWin";
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

pub fn run() -> Result<(), windows_service::Error> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
}

define_windows_service!(ffi_service_main, service_main);

fn service_main(_arguments: Vec<OsString>) {
    if let Err(_e) = run_service() {
        // Handle error
    }
}

struct ServiceStateData {
    config: Config,
    fired_shutdown: Option<NaiveDate>,
    fired_notifs: HashSet<String>,
}

fn run_service() -> Result<(), windows_service::Error> {
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();

    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Stop => {
                let _ = shutdown_tx.send(());
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)?;

    status_handle.set_service_status(ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    let state = Arc::new(Mutex::new(ServiceStateData {
        config: Config::load(),
        fired_shutdown: None,
        fired_notifs: HashSet::new(),
    }));

    let state_clone = state.clone();
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(15));

            let (today, hm, cfg) = {
                let mut data = state_clone.lock().unwrap();
                data.config = Config::load();
                let now = Local::now();
                (now.date_naive(), now.format("%H:%M").to_string(), data.config.clone())
            };

            // ── Grayscale State ──────────────────────────────────────────────
            let desired_grayscale = if cfg.grayscale_enabled {
                if cfg.grayscale_time < cfg.grayscale_disable_time {
                    // e.g. 22:00 to 07:00
                    hm >= cfg.grayscale_time && hm < cfg.grayscale_disable_time
                } else {
                    // e.g. 22:00 to 07:00 (crosses midnight)
                    hm >= cfg.grayscale_time || hm < cfg.grayscale_disable_time
                }
            } else {
                false
            };
            send_ipc(IpcCommand::SetGrayscale(desired_grayscale));

            // ── Shutdown notifications & shutdown ──────────────────────────
            if cfg.shutdown_enabled {
                if let Ok(sd_t) = NaiveTime::parse_from_str(&cfg.shutdown_time, "%H:%M") {
                    for notif in &cfg.notifications {
                        let fire_t = sd_t - chrono::Duration::minutes(notif.minutes_before as i64);
                        let fire_hm = fire_t.format("%H:%M").to_string();

                        if hm == fire_hm {
                            let key = format!("{}-{}", today, notif.minutes_before);
                            let mut data = state_clone.lock().unwrap();
                            if !data.fired_notifs.contains(&key) {
                                data.fired_notifs.insert(key);
                                send_ipc(IpcCommand::ShowNotification {
                                    title: "Shutdown Warning".to_string(),
                                    message: notif.message.clone(),
                                });
                            }
                        }
                    }

                    if cfg.shutdown_time == hm {
                        let mut data = state_clone.lock().unwrap();
                        if data.fired_shutdown != Some(today) {
                            data.fired_shutdown = Some(today);
                            send_ipc(IpcCommand::ShowNotification {
                                title: "Greyscale Timer".to_string(),
                                message: "Shutting down in 60 seconds…".to_string(),
                            });
                            do_shutdown();
                        }
                    }
                }
            }
        }
    });

    let _ = shutdown_rx.recv();

    status_handle.set_service_status(ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    Ok(())
}

fn send_ipc(cmd: IpcCommand) {
    if let Ok(json) = serde_json::to_vec(&cmd) {
        if let Ok(mut file) = OpenOptions::new().write(true).open(PIPE_NAME) {
            let _ = file.write_all(&json);
        }
    }
}

fn do_shutdown() {
    let _ = std::process::Command::new("shutdown")
        .args(["/s", "/f", "/t", "60"])
        .spawn();
}
