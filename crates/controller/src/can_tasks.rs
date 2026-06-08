use brewtech_core::can_protocol::*;
use embassy_futures::select::{select, Either};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::{Receiver, Sender},
    mutex::Mutex,
};
use embassy_time::{Duration, Ticker};
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
    ack_sender: Sender<'static, CriticalSectionRawMutex, u32, 1>,
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

        match can_id.msg_type {
            MSG_TYPE_TEMPERATURE => {
                if let Some(reading) = decode_float(data)
                    .map(|celsius| SensorReading::Temperature { node_id, celsius })
                {
                    sender.try_send(reading).ok();
                }
            }
            MSG_TYPE_DENSITY => {
                if let Some(reading) =
                    decode_float(data).map(|sg| SensorReading::Density { node_id, sg })
                {
                    sender.try_send(reading).ok();
                }
            }
            MSG_TYPE_CALIBRATION_ACK => {
                if let Some(ack) = decode_uint32(data) {
                    if ack != ACK_TYPE_NONE {
                        log::info!("[density #{}] calibration_ack raw={}", node_id, ack);
                        ack_sender.try_send(ack).ok();
                    }
                }
            }
            _ => {}
        }
    }
}

#[embassy_executor::task]
pub async fn can_tx_task(
    mut tx: TwaiTx<'static, Async>,
    cmd_receiver: Receiver<'static, CriticalSectionRawMutex, f32, 1>,
    node_id: u8,
) {
    let probe_data = [0u8];
    let probe_id = CanId {
        priority: PRIORITY_MEDIUM,
        sender_node_type: NODE_TYPE_PLC,
        receiver_node_type: NODE_TYPE_DENSITY_SENSOR,
        secondary_node_id: node_id,
        msg_type: MSG_TYPE_START_MEASUREMENT_CMD,
    }
    .to_u32();
    let calib_id = CanId {
        priority: PRIORITY_HIGH,
        sender_node_type: NODE_TYPE_PLC,
        receiver_node_type: NODE_TYPE_DENSITY_SENSOR,
        secondary_node_id: node_id,
        msg_type: MSG_TYPE_CALIBRATION_CMD,
    }
    .to_u32();

    let mut probe_ticker = Ticker::every(Duration::from_secs(5 * 60));

    loop {
        match select(probe_ticker.next(), cmd_receiver.receive()).await {
            Either::First(_) => {
                if let Some(frame) =
                    EspTwaiFrame::new(ExtendedId::new(probe_id).unwrap(), &probe_data)
                {
                    if let Err(e) = tx.transmit_async(&frame).await {
                        log::warn!("CAN tx error probing node {}: {:?}", node_id, e);
                    }
                }
            }
            Either::Second(sg) => {
                let calib_data = encode_float(sg);
                if let Some(frame) =
                    EspTwaiFrame::new(ExtendedId::new(calib_id).unwrap(), &calib_data)
                {
                    if let Err(e) = tx.transmit_async(&frame).await {
                        log::warn!("CAN tx error calibrating node {}: {:?}", node_id, e);
                    } else {
                        log::info!("[density #{}] calibration cmd sent sg={:.4}", node_id, sg);
                    }
                }
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
