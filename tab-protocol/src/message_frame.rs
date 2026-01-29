use nix::errno::Errno;
use nix::sys::socket::{ControlMessage, ControlMessageOwned, MsgFlags, recvmsg, sendmsg};
use serde::Serialize;
use std::io::{ErrorKind, IoSlice, IoSliceMut};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;

use crate::{HelloPayload, MessageHeader, PROTOCOL_VERSION, ProtocolError};

/// Raw framed Tab message: header line + payload line (strings) plus optional FDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabMessageFrame {
	pub header: MessageHeader,
	pub payload: Option<String>,
	pub fds: Vec<RawFd>,
}
fn would_block_err() -> std::io::Error {
	std::io::Error::new(ErrorKind::WouldBlock, ProtocolError::WouldBlock)
}
impl TabMessageFrame {
	/// Write a framed TabMessageFrame to the provided stream using sendmsg/SCM_RIGHTS.
	pub fn encode_and_send(&self, stream: &impl AsRawFd) -> Result<(), ProtocolError> {
		let encoded = self.serialize();
		let iov = [IoSlice::new(encoded.as_bytes())];
		let cmsg = if self.fds.is_empty() {
			vec![]
		} else {
			vec![ControlMessage::ScmRights(&self.fds)]
		};
		sendmsg::<()>(stream.as_raw_fd(), &iov, &cmsg, MsgFlags::empty(), None)?;
		Ok(())
	}
	pub fn serialize(&self) -> String {
		let header_line = self.header.0.trim_end();
		let payload_line = self
			.payload
			.as_ref()
			.map(|p| p.trim_end_matches('\n'))
			.unwrap_or_else(|| "\0\0\0\0");

		format!("{header_line}\n{payload_line}\n")
	}

	/// Non-blocking version of [`read_framed`]
	#[cfg(feature = "async")]
	pub async fn read_frame_from_async_fd<T: AsRawFd>(
		fd: &tokio::io::unix::AsyncFd<T>,
	) -> Result<Self, ProtocolError> {
		loop {
			let mut guard = fd.readable().await?;
			if let Ok(result) = guard.try_io(|_| match Self::read_framed(fd.get_ref()) {
				Err(ProtocolError::WouldBlock) => Err(would_block_err()),
				def => Ok(def),
			}) {
				break result?;
			} else {
				continue;
			}
		}
	}
	/// Sends a message asynchronously
	#[cfg(feature = "async")]
	pub async fn send_frame_to_async_fd<T: AsRawFd>(
		&self,
		fd: &tokio::io::unix::AsyncFd<T>,
	) -> Result<(), ProtocolError> {
		let packet = loop {
			let mut guard = fd.writable().await?;
			if let Ok(result) = guard.try_io(|_| match self.encode_and_send(fd) {
				Err(ProtocolError::WouldBlock) => Err(would_block_err()),
				def => Ok(def),
			}) {
				break result?;
			} else {
				continue;
			}
		}?;
		return Ok(packet);
	}
	/// Read one Tab message frame using recvmsg/SCM_RIGHTS.
	pub fn read_framed(stream: &impl AsRawFd) -> Result<Self, ProtocolError> {
		// Enough for two short lines.
		let mut buf = [0u8; 4096];
		// Allow up to 8 incoming FDs per message; Tab v1 uses far fewer.
		let mut cmsg_space = nix::cmsg_space!([RawFd; 8]);
		let mut iov = [IoSliceMut::new(&mut buf)];

		let msg = loop {
			match recvmsg::<()>(
				stream.as_raw_fd(),
				&mut iov,
				Some(&mut cmsg_space),
				MsgFlags::empty(),
			) {
				Err(errno) if errno == Errno::EINTR => continue,
				Err(errno) if errno == Errno::EAGAIN || errno == Errno::EWOULDBLOCK => {
					break Err(ProtocolError::WouldBlock);
				}
				Err(errno) => break Err(ProtocolError::Nix(errno.into())),
				Ok(msg) => break Ok(msg),
			}
		}?;
		if msg.bytes == 0 {
			return Err(ProtocolError::UnexpectedEof);
		}
		if msg.flags.contains(MsgFlags::MSG_TRUNC) {
			return Err(ProtocolError::Truncated);
		}
		let bytes_read = msg.bytes;

		let mut fds = Vec::new();
		let mut c_iter = msg.cmsgs()?;
		while let Some(cmsg) = c_iter.next() {
			if let ControlMessageOwned::ScmRights(rights) = cmsg {
				fds.extend(rights);
			}
		}
		let _ = msg; // release borrow on iov/buf

		let data = &iov[0][..bytes_read];

		let Some((frame, used)) = Self::parse_from_bytes(data, fds)? else {
			return Err(ProtocolError::UnexpectedEof);
		};
		if used < data.len() {
			if data[used..].iter().any(|b| *b != 0) {
				return Err(ProtocolError::TrailingData);
			}
		}
		Ok(frame)
	}

	pub(crate) fn expect_payload_json<'a, T>(&'a self) -> Result<T, ProtocolError>
	where
		T: serde::Deserialize<'a>,
	{
		if let Some(payload) = &self.payload {
			serde_json::from_str(payload.as_str()).map_err(ProtocolError::from)
		} else {
			Err(ProtocolError::ExpectedPayload)
		}
	}
	pub fn json(header: impl Into<MessageHeader>, payload: impl Serialize) -> Self {
		Self {
			header: header.into(),
			payload: Some(serde_json::to_string(&payload).unwrap()),
			fds: Vec::new(),
		}
	}

	pub fn raw(header: impl Into<MessageHeader>, body: impl Into<String>) -> Self {
		Self {
			header: header.into(),
			payload: Some(body.into()),
			fds: Vec::new(),
		}
	}

	pub fn no_payload(header: impl Into<MessageHeader>) -> Self {
		Self {
			header: header.into(),
			payload: None,
			fds: Vec::new(),
		}
	}
	pub fn hello(server: impl Into<String>) -> Self {
		let payload = HelloPayload {
			server: server.into(),
			protocol: PROTOCOL_VERSION.to_string(),
		};
		let json = serde_json::to_value(payload).expect("HelloPayload is serializable");
		Self::json("hello", json)
	}

	pub fn expect_n_fds(&self, amount: u32) -> Result<(), ProtocolError> {
		let found = self.fds.len() as u32;
		if found == amount {
			Ok(())
		} else {
			Err(ProtocolError::ExpectedFds {
				expected: amount,
				found,
			})
		}
	}

	pub fn parse_from_bytes(
		bytes: &[u8],
		fds: Vec<RawFd>,
	) -> Result<Option<(Self, usize)>, ProtocolError> {
		let Some(first_nl) = bytes.iter().position(|b| *b == b'\n') else {
			return Ok(None);
		};
		let Some(second_rel) = bytes[first_nl + 1..].iter().position(|b| *b == b'\n') else {
			return Ok(None);
		};
		let second_nl = first_nl + 1 + second_rel;
		let header_bytes = &bytes[..first_nl];
		let payload_bytes = &bytes[first_nl + 1..second_nl];
		let consumed = second_nl + 1;
		let frame = Self::from_lines(header_bytes, payload_bytes, fds)?;
		Ok(Some((frame, consumed)))
	}

	fn from_lines(
		header_bytes: &[u8],
		payload_bytes: &[u8],
		fds: Vec<RawFd>,
	) -> Result<Self, ProtocolError> {
		let header = String::from_utf8(header_bytes.to_vec())?;
		let payload_str = String::from_utf8(payload_bytes.to_vec())?;
		Ok(Self {
			header: header.into(),
			payload: if payload_str == "\0\0\0\0" {
				None
			} else {
				Some(payload_str)
			},
			fds,
		})
	}
}
