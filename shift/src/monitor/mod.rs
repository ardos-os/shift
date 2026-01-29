use crate::define_id_type;

define_id_type!(Monitor, "mon_");
#[derive(Debug, Clone)]
pub struct Monitor {
	pub id: MonitorId,
	pub width: i32,
	pub height: i32,
	pub refresh_rate: u32,
	pub name: String,
}
