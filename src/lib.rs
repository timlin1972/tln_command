use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use anstream::println;
use owo_colors::OwoColorize as _;
use serde::{Deserialize, Serialize};

use common::{cfg, plugin, utils};

const MODULE: &str = "command";
const REPLY_DELAY: u64 = 100;
const SHOW: &str = r#"
action plugin command shell start
    Start shell

action plugin command cmd {cmd}
    Issue command

action plugin command shell stop
    Stop shell

action plugin command dest HomeUbuntu
    Set the output destination device ('HomeUbuntu')

action plugin command remote remote pi5
    Set the remote device to receive the command

action plugin command remote dest HomeUbuntu
    Ask the remote device to set the output destination ('HomeUbuntu')

action plugin command remote shell start
    Ask the remote device to start shell

action plugin command remote cmd {cmd}
    Ask the remote device to issue command

action plugin command remote shell stop
    Ask the remote device to stop shell
"#;

#[derive(Serialize, Deserialize, Debug)]
struct Report {
    topic: String,
    payload: String,
}

pub struct Plugin {
    tx: crossbeam_channel::Sender<String>,
    child: Option<Arc<Mutex<Child>>>,
    stdin: Option<ChildStdin>,
    dest: Arc<Mutex<Option<String>>>,
    remote: Option<String>,
}

impl Plugin {
    pub fn new(tx: &crossbeam_channel::Sender<String>) -> Plugin {
        println!("[{}] Loading...", MODULE.blue());

        Plugin {
            tx: tx.clone(),
            child: None,
            stdin: None,
            dest: Arc::new(Mutex::new(None)),
            remote: None,
        }
    }

    fn stdout_task(&mut self, child: &mut Child) {
        let stdout = child.stdout.take().expect("Failed to open stdout");
        let stdout_reader = BufReader::new(stdout);
        let tx_clone = self.tx.clone();
        let dest_clone = Arc::clone(&self.dest);
        std::thread::spawn(move || {
            for line in stdout_reader.lines() {
                match line {
                    Ok(output) => {
                        let dest = dest_clone.lock().unwrap();
                        if dest.is_none() {
                            println!("{output}");
                        } else {
                            let dest = dest.clone().unwrap();
                            let report = Report {
                                topic: format!("tln/{dest}/echo"),
                                payload: output.to_owned(),
                            };
                            let json_string = serde_json::to_string(&report).unwrap();

                            tx_clone
                                .send(format!("send plugin mqtt report '{json_string}'"))
                                .unwrap();
                            std::thread::sleep(std::time::Duration::from_millis(REPLY_DELAY));
                        }
                    }
                    Err(e) => {
                        eprintln!("Error reading stdout: {}", e);
                        break;
                    }
                }
            }
        });
    }

    fn stderr_task(&mut self, child: &mut Child) {
        let stderr = child.stderr.take().expect("Failed to open stderr");
        let stderr_reader = BufReader::new(stderr);
        let tx_clone = self.tx.clone();
        let dest_clone = Arc::clone(&self.dest);
        std::thread::spawn(move || {
            for line in stderr_reader.lines() {
                match line {
                    Ok(output) => {
                        let dest = dest_clone.lock().unwrap();
                        if dest.is_none() {
                            println!("{output}");
                        } else {
                            let dest = dest.clone().unwrap();
                            let report = Report {
                                topic: format!("tln/{dest}/echo"),
                                payload: output.to_owned(),
                            };
                            let json_string = serde_json::to_string(&report).unwrap();

                            tx_clone
                                .send(format!("send plugin mqtt report '{json_string}'"))
                                .unwrap();
                            std::thread::sleep(std::time::Duration::from_millis(REPLY_DELAY));
                        }
                    }
                    Err(e) => {
                        eprintln!("Error reading stderr: {}", e);
                        break;
                    }
                }
            }
        });
    }
}

impl plugin::Plugin for Plugin {
    fn name(&self) -> &str {
        MODULE
    }

    fn show(&mut self) -> String {
        println!("[{}]", MODULE.blue());

        let mut show = String::new();
        show += SHOW;

        println!("{show}");

        show
    }

    fn status(&mut self) -> String {
        println!("[{}]", MODULE.blue());

        let mut status = String::new();

        status += &format!(
            "child: {}\n",
            if self.child.is_some() { "yes" } else { "no" }
        );
        status += &format!(
            "stdin: {}\n",
            if self.stdin.is_some() { "yes" } else { "no" }
        );

        let dest = self.dest.lock().unwrap();

        status += &format!(
            "dest: {} (the output destination)\n",
            if dest.is_some() {
                dest.clone().unwrap()
            } else {
                "None".to_owned()
            }
        );

        status += &format!(
            "remote: {} (control remotely)\n",
            if self.remote.is_some() {
                self.remote.clone().unwrap()
            } else {
                "None".to_owned()
            }
        );

        println!("{status}");

        status
    }

    fn action(&mut self, action: &str, data: &str, data2: &str) -> String {
        match action {
            "shell" => {
                match data {
                    "start" => {
                        let mut child = Command::new(cfg::get_shell())
                            .stdin(Stdio::piped()) // 輸入管道
                            .stdout(Stdio::piped()) // 標準輸出管道
                            .stderr(Stdio::piped()) // 錯誤輸出管道
                            .spawn()
                            .expect("Failed to start shell");

                        let stdin = child.stdin.take().expect("Failed to open stdin");

                        self.stdout_task(&mut child);
                        self.stderr_task(&mut child);

                        self.child = Some(Arc::new(Mutex::new(child)));

                        self.stdin = Some(stdin);
                    }
                    "stop" => {
                        writeln!(self.stdin.as_mut().unwrap(), "exit")
                            .expect("Failed to write to shell");

                        self.stdin = None;
                        self.child = None;

                        let mut dest_clone = self.dest.lock().unwrap();
                        *dest_clone = None;
                    }
                    _ => (),
                }
            }
            "cmd" => {
                if self.stdin.is_none() {
                    println!("Please start shell first (action plugin command shell start)");
                } else {
                    writeln!(self.stdin.as_mut().unwrap(), "{}", data)
                        .expect("Failed to write to shell");
                }
            }
            "dest" => {
                let mut dest_clone = self.dest.lock().unwrap();
                *dest_clone = Some(data.to_owned());
            }
            "remote" => match data {
                "shell" => {
                    if self.remote.is_none() {
                        println!("Please set the remote device first (action plugin command remote remote device_name)");
                    } else {
                        match data2 {
                            "start" => {
                                let report = Report {
                                    topic: format!("tln/{}/send", self.remote.clone().unwrap()),
                                    payload: utils::encrypt("action plugin command shell start"),
                                };
                                let json_string = serde_json::to_string(&report).unwrap();

                                self.tx
                                    .send(format!("send plugin mqtt report '{json_string}'"))
                                    .unwrap();
                            }
                            "stop" => {
                                let report = Report {
                                    topic: format!("tln/{}/send", self.remote.clone().unwrap()),
                                    payload: utils::encrypt("action plugin command shell stop"),
                                };
                                let json_string = serde_json::to_string(&report).unwrap();

                                self.tx
                                    .send(format!("send plugin mqtt report '{json_string}'"))
                                    .unwrap();
                            }
                            _ => (),
                        }
                    }
                }
                "cmd" => {
                    if self.remote.is_none() {
                        println!("Please set the remote device first (action plugin command remote remote {{device_name}})");
                    } else {
                        let report = Report {
                            topic: format!("tln/{}/send", self.remote.clone().unwrap()),
                            payload: utils::encrypt(&format!(
                                "action plugin command cmd '{data2}'"
                            )),
                        };
                        let json_string = serde_json::to_string(&report).unwrap();

                        self.tx
                            .send(format!("send plugin mqtt report '{json_string}'"))
                            .unwrap();
                    }
                }
                "dest" => {
                    if self.remote.is_none() {
                        println!("Please set the remote device first (action plugin command remote remote {{device_name}})");
                    } else {
                        let report = Report {
                            topic: format!("tln/{}/send", self.remote.clone().unwrap()),
                            payload: utils::encrypt(&format!(
                                "action plugin command dest '{data2}'"
                            )),
                        };
                        let json_string = serde_json::to_string(&report).unwrap();

                        self.tx
                            .send(format!("send plugin mqtt report '{json_string}'"))
                            .unwrap();
                    }
                }
                "remote" => {
                    self.remote = Some(data2.to_owned());
                }
                _ => (),
            },
            _ => (),
        }

        "send".to_owned()
    }
}

#[no_mangle]
pub extern "C" fn create_plugin(
    tx: &crossbeam_channel::Sender<String>,
) -> *mut plugin::PluginWrapper {
    let plugin = Box::new(Plugin::new(tx));
    Box::into_raw(Box::new(plugin::PluginWrapper::new(plugin)))
}

/// # Safety
#[no_mangle]
pub unsafe extern "C" fn unload_plugin(wrapper: *mut plugin::PluginWrapper) {
    if !wrapper.is_null() {
        unsafe {
            let _ = Box::from_raw(wrapper);
        }
    }
}
