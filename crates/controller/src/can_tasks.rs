use brewtech_core::can_protocol::*;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Sender, mutex::Mutex};
use embassy_time::{Duration, Timer};
use embedded_can::Frame as _;
use esp_hal::{
    twai::{EspTwaiFrame, ExtendedId, TwaiRx, TwaiTx},
    Async,
};

use crate::{SensorReading, SensorState};

#[embassy_executor::task]
pub async fn can_rx_task(
    mut rx: TwaiRx<'static, Async>,
    sender: Sender<'static, CriticalSectionRawMutex, SensorReading, 16>,
    node_id: u8,
) {
    loop {
        let frame = match rx.receive_async().await {
            Ok(f) => f,
            Err(e) => {
                log::warn!("CAN rx error: {:?}", e);
                continue;
            }
        };

        let raw_id = match frame.id() {
            embedded_can::Id::Extended(ext) => ext.as_raw(),
            embedded_can::Id::Standard(_) => continue,
        };

        let can_id = CanId::from_u32(raw_id);
        if can_id.sender_node_type != NODE_TYPE_DENSITY_SENSOR
            || can_id.secondary_node_id != node_id
        {
            continue;
        }

        let data = frame.data();

        let reading = match can_id.msg_type {
            MSG_TYPE_TEMPERATURE => {
                decode_float(data).map(|celsius| SensorReading::Temperature { node_id, celsius })
            }
            MSG_TYPE_DENSITY => decode_float(data).map(|sg| SensorReading::Density { node_id, sg }),
            _ => None,
        };

        if let Some(r) = reading {
            sender.try_send(r).ok();
        }
    }
}

#[embassy_executor::task]
pub async fn density_probe_task(mut tx: TwaiTx<'static, Async>, node_id: u8) {
    let probe_data = [0u8];
    let raw_id = CanId {
        priority: PRIORITY_MEDIUM,
        sender_node_type: NODE_TYPE_PLC,
        receiver_node_type: NODE_TYPE_DENSITY_SENSOR,
        secondary_node_id: node_id,
        msg_type: MSG_TYPE_START_MEASUREMENT_CMD,
    }
    .to_u32();

    loop {
        Timer::after(Duration::from_secs(30)).await;
        if let Some(frame) = EspTwaiFrame::new(ExtendedId::new(raw_id).unwrap(), &probe_data) {
            if let Err(e) = tx.transmit_async(&frame).await {
                log::warn!("CAN tx error probing node {}: {:?}", node_id, e);
            }
        }
    }
}

#[embassy_executor::task]
pub async fn sensor_log_task(
    receiver: embassy_sync::channel::Receiver<'static, CriticalSectionRawMutex, SensorReading, 16>,
    state: &'static Mutex<CriticalSectionRawMutex, SensorState>,
    node_id: u8,
) {
    loop {
        let reading = receiver.receive().await;
        match reading {
            SensorReading::Temperature {
                node_id: id,
                celsius,
            } => {
                log::info!("[density #{}] temp  = {:.2} °C", id, celsius);
            }
            SensorReading::Density { node_id: id, sg } => {
                log::info!("[density #{}] sg    = {:.4}", id, sg);
            }
        }
        let mut s = state.lock().await;
        match reading {
            SensorReading::Temperature {
                node_id: id,
                celsius,
            } if id == node_id => {
                s.temperature = Some(celsius);
            }
            SensorReading::Density { node_id: id, sg } if id == node_id => {
                s.density = Some(sg);
            }
            _ => {}
        }
    }
}
