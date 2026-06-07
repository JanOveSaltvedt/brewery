use brewtech_core::can_protocol::*;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Sender};
use embassy_time::{Duration, Timer};
use embedded_can::Frame as _;
use esp_hal::{
    Async,
    twai::{EspTwaiFrame, ExtendedId, TwaiRx, TwaiTx},
};

use crate::SensorReading;

#[embassy_executor::task]
pub async fn can_rx_task(
    mut rx: TwaiRx<'static, Async>,
    sender: Sender<'static, CriticalSectionRawMutex, SensorReading, 16>,
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
        if can_id.sender_node_type != NODE_TYPE_DENSITY_SENSOR {
            continue;
        }

        let data = frame.data();
        let node_id = can_id.secondary_node_id;

        let reading = match can_id.msg_type {
            MSG_TYPE_TEMPERATURE => decode_float(data)
                .map(|celsius| SensorReading::Temperature { node_id, celsius }),
            MSG_TYPE_DENSITY => decode_float(data)
                .map(|sg| SensorReading::Density { node_id, sg }),
            _ => None,
        };

        if let Some(r) = reading {
            sender.try_send(r).ok();
        }
    }
}

#[embassy_executor::task]
pub async fn density_probe_task(mut tx: TwaiTx<'static, Async>) {
    // Sub-index only, no payload — measurement cmd needs no data.
    let probe_data = [0u8];
    loop {
        Timer::after(Duration::from_secs(5)).await;
        for node_id in 0..MAX_NODES as u8 {
            let raw_id = CanId {
                priority: PRIORITY_MEDIUM,
                sender_node_type: NODE_TYPE_PLC,
                receiver_node_type: NODE_TYPE_DENSITY_SENSOR,
                secondary_node_id: node_id,
                msg_type: MSG_TYPE_START_MEASUREMENT_CMD,
            }
            .to_u32();

            if let Some(frame) =
                EspTwaiFrame::new(ExtendedId::new(raw_id).unwrap(), &probe_data)
            {
                if let Err(e) = tx.transmit_async(&frame).await {
                    log::warn!("CAN tx error probing node {}: {:?}", node_id, e);
                }
            }
        }
    }
}

#[embassy_executor::task]
pub async fn sensor_log_task(
    receiver: embassy_sync::channel::Receiver<'static, CriticalSectionRawMutex, SensorReading, 16>,
) {
    loop {
        match receiver.receive().await {
            SensorReading::Temperature { node_id, celsius } => {
                log::info!("[density #{}] temp  = {:.2} °C", node_id, celsius);
            }
            SensorReading::Density { node_id, sg } => {
                log::info!("[density #{}] sg    = {:.4}", node_id, sg);
            }
        }
    }
}
