#![allow(dead_code)]

// Priorities (2-bit field, lower value = higher priority)
pub const PRIORITY_HIGH: u8 = 0;
pub const PRIORITY_MEDIUM: u8 = 1;

// Node types
pub const NODE_TYPE_FCS_F_IO: u8 = 2;
pub const NODE_TYPE_PRESSURE_SENSOR: u8 = 3;
pub const NODE_TYPE_DENSITY_SENSOR: u8 = 4;
pub const NODE_TYPE_LEVEL_SENSOR: u8 = 5;
pub const NODE_TYPE_AGITATOR: u8 = 6;
pub const NODE_TYPE_PLC: u8 = 8;

// Message types
pub const MSG_TYPE_SEMANTIC_VERSION: u8 = 1;
pub const MSG_TYPE_TEMPERATURE: u8 = 12;
pub const MSG_TYPE_PRESSURE: u8 = 13;
pub const MSG_TYPE_DENSITY: u8 = 14;
pub const MSG_TYPE_LEVEL: u8 = 16;
pub const MSG_TYPE_RPM: u8 = 17;
pub const MSG_TYPE_DCC: u8 = 18;
pub const MSG_TYPE_ACC: u8 = 19;
pub const MSG_TYPE_PWM: u8 = 27;
pub const MSG_TYPE_CALIBRATION_CMD: u8 = 28;
pub const MSG_TYPE_CALIBRATION_ACK: u8 = 29;
pub const MSG_TYPE_START_MEASUREMENT_CMD: u8 = 33;
pub const MSG_TYPE_NODE_ID: u8 = 36;

// Calibration acknowledgement values (carried as big-endian u32 in bytes 1–4)
pub const ACK_TYPE_NONE: u32 = 0;
pub const ACK_TYPE_CALIBRATING: u32 = 1;
pub const ACK_TYPE_OK: u32 = 2;
pub const ACK_TYPE_ERROR: u32 = 3;

pub const MAX_NODES: usize = 8;

/// Decoded fields of a 29-bit extended CAN identifier.
///
/// Layout (MSB → LSB):
///   [28:27] priority · [26:19] senderNodeType · [18:11] receiverNodeType
///   [10:8]  secondaryNodeId · [7:0] msgType
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CanId {
    pub priority: u8,
    pub sender_node_type: u8,
    pub receiver_node_type: u8,
    pub secondary_node_id: u8,
    pub msg_type: u8,
}

impl CanId {
    pub fn from_u32(id: u32) -> Self {
        Self {
            priority:           ((id >> 27) & 0x03) as u8,
            sender_node_type:   ((id >> 19) & 0xFF) as u8,
            receiver_node_type: ((id >> 11) & 0xFF) as u8,
            secondary_node_id:  ((id >> 8)  & 0x07) as u8,
            msg_type:            (id        & 0xFF) as u8,
        }
    }

    pub fn to_u32(&self) -> u32 {
        ((self.priority as u32)           << 27)
            | ((self.sender_node_type as u32)   << 19)
            | ((self.receiver_node_type as u32) << 11)
            | ((self.secondary_node_id as u32)  <<  8)
            |  (self.msg_type as u32)
    }
}

/// Encode a float as a 5-byte payload: `[sub_index=0, f32_le_bytes…]`.
pub fn encode_float(value: f32) -> [u8; 5] {
    let b = value.to_le_bytes();
    [0, b[0], b[1], b[2], b[3]]
}

/// Decode a float from a CAN data slice (bytes 1–4, little-endian).
/// Returns `None` if the slice is too short.
pub fn decode_float(data: &[u8]) -> Option<f32> {
    if data.len() < 5 {
        return None;
    }
    Some(f32::from_le_bytes([data[1], data[2], data[3], data[4]]))
}

/// Encode a `u32` as a 5-byte payload: `[sub_index=0, u32_be_bytes…]`.
pub fn encode_uint32(value: u32) -> [u8; 5] {
    [
        0,
        ((value >> 24) & 0xFF) as u8,
        ((value >> 16) & 0xFF) as u8,
        ((value >>  8) & 0xFF) as u8,
         (value        & 0xFF) as u8,
    ]
}

/// Decode a big-endian `u32` from bytes 1–4 of a CAN data slice.
/// Returns `None` if the slice is too short.
pub fn decode_uint32(data: &[u8]) -> Option<u32> {
    if data.len() < 5 {
        return None;
    }
    Some(
        ((data[1] as u32) << 24)
            | ((data[2] as u32) << 16)
            | ((data[3] as u32) << 8)
            |  (data[4] as u32),
    )
}
