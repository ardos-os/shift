use std::{
	ffi::{CStr, CString},
	os::fd::AsRawFd,
	os::raw::c_char,
	collections::VecDeque,
};

use tab_protocol::{
	InputEventPayload, MonitorInfo, SessionInfo, SessionLifecycle, SessionRole,
	DEFAULT_SOCKET_PATH, ButtonState, AxisOrientation, AxisSource, KeyState,
};

use crate::{TabClient, TabEvent as RustTabEvent};

pub mod connection;
pub mod event;
pub mod frame;
pub mod input;
pub mod monitor;
pub mod session;

// ============================================================================
// OPAQUE HANDLES
// ============================================================================

/// Opaque handle to a TabClient instance
#[repr(C)]
pub struct TabClientHandle {
	inner: Box<TabClient>,
	event_queue: VecDeque<RustTabEvent>,
}
