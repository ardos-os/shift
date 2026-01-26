#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
#[repr(u8)]
pub enum Role {
    Normal = 0,
    Admin = 1,
}
