use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Command {
    SetGrayscale(bool),
    ShowNotification { title: String, message: String },
}

pub const PIPE_NAME: &str = r"\\.\pipe\TheWorldIsGreyShutItWinPipe";
