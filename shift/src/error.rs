use easydrm::EasyDRMError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RenderError {
	#[error("failed to make monitor current: {0}")]
	MakeCurrent(String),
}

#[derive(Debug, Error)]
pub enum ShiftError {
	#[error("tab server error: {0}")]
	TabServer(#[from] tab_server::TabServerError),
	#[error("easydrm error: {0}")]
	EasyDrm(#[from] EasyDRMError),
	#[error("io error: {0}")]
	Io(#[from] std::io::Error),
	#[error("env var error: {0}")]
	EnvVar(#[from] std::env::VarError),
	#[error("render error: {0}")]
	Render(#[from] RenderError),
	#[error("libinput error: {0}")]
	Libinput(String),
}

pub type FrameAck = Vec<(String, String)>;
