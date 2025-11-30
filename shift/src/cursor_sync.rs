use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use easydrm::EasyDRM;
use tab_server::TabServer;
use tracing::debug;

use crate::dma_buf_importer::ExternalTexture;
use crate::output::OutputContext;

/// Manages synchronization of hardware cursor state with the tab-server's
/// per-session/per-monitor cursor state. Uses image hashing to minimize
/// unnecessary buffer re-uploads when only the cursor position changes.
pub struct CursorSync {
	/// Track previous cursor image hashes per (monitor_id, session_id) to detect
	/// when the cursor image itself changes (as opposed to just position).
	/// Key: (monitor_id, session_id), Value: (image_hash, last_position_x, last_position_y)
	state: HashMap<(String, String), (u64, i32, i32)>,
}

impl CursorSync {
	/// Create a new cursor sync manager.
	pub fn new() -> Self {
		Self {
			state: HashMap::new(),
		}
	}

	/// Synchronize hardware cursor state from tab-server.
	///
	/// For the active session:
	/// - If a cursor exists: re-upload buffer when image_hash changes, always update position
	/// - If no cursor exists: clear any tracked state and remove from hardware
	///
	/// Uses the pre-calculated image_hash to avoid expensive buffer uploads
	/// when only the cursor position has changed.
	pub fn sync(
		&mut self,
		server: &TabServer<ExternalTexture>,
		easydrm: &Rc<RefCell<EasyDRM<OutputContext>>>,
	) {
		let active_session = server.active_session_id().map(|s| s.to_string());
		let monitor_infos = server.monitor_infos();

		for monitor_info in monitor_infos {
			let monitor_id = &monitor_info.id;

			// Only sync cursors for the active session
			let Some(session_id) = &active_session else {
				continue;
			};
			let key = (monitor_id.clone(), session_id.clone());

			// Check if a cursor exists for this session/monitor
			if let Some(cursor) = server.get_cursor(monitor_id, session_id) {
				let current_hash = cursor.image_hash();
				let current_x = cursor.position_x();
				let current_y = cursor.position_y();

				let state_entry = self.state.get(&key).copied();
                if state_entry.is_none() {
                    // No previous state; insert default
                    self.state
                        .insert(key.clone(), (0, i32::MIN, i32::MIN));
    
                };
				// let (prev_hash, prev_x, prev_y) = *state_entry;

				// Extract handle first while only doing immutable borrow
				let handle = {
					let edrm = easydrm.borrow();
					edrm.monitors()
						.find(|m| {
							m.context()
								.monitor_id()
								.is_some_and(|id| id == monitor_id.as_str())
						})
						.map(|m| m.connector_id())
				};
                
				if let Some(handle) = handle {
					// Now use mutable borrow for method calls
					let mut edrm = easydrm.borrow_mut();

					// If the image hash changed, re-upload the cursor buffer
					if state_entry.is_none_or(|entry| current_hash != entry.0) {
						debug!(
							monitor_id = %monitor_id,
							session_id = %session_id,
							width = cursor.width(),
							height = cursor.height(),
							"Uploading cursor buffer to hardware"
						);
						if let Err(e) = edrm.set_cursor_buffer(
							handle,
							cursor.width() as usize,
							cursor.height() as usize,
							cursor.image(),
						) {
							tracing::warn!(
								monitor_id = %monitor_id,
								session_id = %session_id,
								%e,
								"Failed to set cursor buffer"
							);
						}
					}

					// Always update cursor position (cheap operation)
					if state_entry.is_none_or(|entry| current_x != entry.1 || current_y != entry.2) {
						debug!(
							monitor_id = %monitor_id,
							session_id = %session_id,
							x = current_x,
							y = current_y,
							"Updating cursor position"
						);
						if let Err(e) = edrm.set_cursor_position(handle, current_x as i64, current_y as i64) {
							tracing::warn!(
								monitor_id = %monitor_id,
								session_id = %session_id,
								%e,
								"Failed to set cursor position"
							);
						}
					}

					// Update the tracked state
					self.state.insert(key.clone(), (current_hash, current_x, current_y));
				}
			} else {
				// No cursor for this session/monitor; remove any tracked state
				if let Some((_, _, _)) = self.state.remove(&key) {
					debug!(
						monitor_id = %monitor_id,
						session_id = %session_id,
						"Cursor removed; clearing hardware state"
					);

					// Extract handle first while only doing immutable borrow
					let handle = {
						let edrm = easydrm.borrow();
						edrm.monitors()
							.find(|m| {
								m.context()
									.monitor_id()
									.is_some_and(|id| id == monitor_id.as_str())
							})
							.map(|m| m.connector_id())
					};

					if let Some(handle) = handle {
						let mut edrm = easydrm.borrow_mut();
						if let Err(e) = edrm.remove_cursor(handle) {
							tracing::warn!(
								monitor_id = %monitor_id,
								session_id = %session_id,
								%e,
								"Failed to remove cursor"
							);
						}
					}
				}
			}
		}
	}
}

impl Default for CursorSync {
	fn default() -> Self {
		Self::new()
	}
}
