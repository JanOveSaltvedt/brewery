//! Test-bench firmware: simulates a BrewTools density/temperature sensor at node ID 0.
//!
//! Behaviour:
//!   - Broadcasts temperature every 500 ms.
//!   - Responds to MSG_TYPE_START_MEASUREMENT_CMD with a density reading.
//!
//! Wiring: same as controller — GPIO0 = CRX, GPIO1 = CTX via SN65HVD230.

#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

use brewtech_core::can_protocol::*;
use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Ticker};
use embedded_can::Frame as _;
use esp_hal::{
    Async,
    interrupt::software::SoftwareInterruptControl,
    timer::timg::TimerGroup,
    twai::{BaudRate, EspTwaiFrame, ExtendedId, TwaiConfiguration, TwaiMode, TwaiRx, TwaiTx},
};
use log::LevelFilter;

use esp_backtrace as _;

esp_bootloader_esp_idf::esp_app_desc!();

const OWN_NODE_ID: u8 = 0;
const SIMULATED_TEMP_C: f32 = 20.5;
const SIMULATED_DENSITY_SG: f32 = 1.045;

// Signals the tx task that a density measurement has been requested.
static DENSITY_REQUESTED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[esp_hal::main]
async fn main(spawner: Spawner) {
    esp_println::logger::init_logger(LevelFilter::Info);

    let p = esp_hal::init(esp_hal::Config::default());

    let sw_int = SoftwareInterruptControl::new(p.SW_INTERRUPT);
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    let twai = TwaiConfiguration::new(
        p.TWAI0,
        p.GPIO0, // rx
        p.GPIO1, // tx
        BaudRate::B1000K,
        TwaiMode::Normal,
    )
    .into_async()
    .start();

    let (rx, tx) = twai.split();

    spawner.spawn(sensor_rx_task(rx).unwrap());
    spawner.spawn(sensor_tx_task(tx).unwrap());

    log::info!("test-can-devices ready — simulating density sensor @ node {}", OWN_NODE_ID);
}

// ---------------------------------------------------------------------------
// RX: listen for probe commands and signal the tx task.
// ---------------------------------------------------------------------------

#[embassy_executor::task]
async fn sensor_rx_task(mut rx: TwaiRx<'static, Async>) {
    loop {
        let frame = match rx.receive_async().await {
            Ok(f) => f,
            Err(e) => {
                log::warn!("rx error: {:?}", e);
                continue;
            }
        };

        let raw_id = match frame.id() {
            embedded_can::Id::Extended(ext) => ext.as_raw(),
            embedded_can::Id::Standard(_) => continue,
        };

        let can_id = CanId::from_u32(raw_id);
        if can_id.receiver_node_type == NODE_TYPE_DENSITY_SENSOR
            && can_id.secondary_node_id == OWN_NODE_ID
            && can_id.msg_type == MSG_TYPE_START_MEASUREMENT_CMD
        {
            log::debug!("density probe received from node type {}", can_id.sender_node_type);
            DENSITY_REQUESTED.signal(());
        }
    }
}

// ---------------------------------------------------------------------------
// TX: broadcast temperature on a 500 ms ticker; send density on request.
// ---------------------------------------------------------------------------

#[embassy_executor::task]
async fn sensor_tx_task(mut tx: TwaiTx<'static, Async>) {
    let mut ticker = Ticker::every(Duration::from_millis(500));
    loop {
        match select(ticker.next(), DENSITY_REQUESTED.wait()).await {
            Either::First(_) => send_measurement(&mut tx, MSG_TYPE_TEMPERATURE, SIMULATED_TEMP_C).await,
            Either::Second(_) => send_measurement(&mut tx, MSG_TYPE_DENSITY, SIMULATED_DENSITY_SG).await,
        }
    }
}

async fn send_measurement(tx: &mut TwaiTx<'static, Async>, msg_type: u8, value: f32) {
    let raw_id = CanId {
        priority: PRIORITY_HIGH,
        sender_node_type: NODE_TYPE_DENSITY_SENSOR,
        receiver_node_type: NODE_TYPE_PLC,
        secondary_node_id: OWN_NODE_ID,
        msg_type,
    }
    .to_u32();

    let data = encode_float(value);
    if let Some(frame) = EspTwaiFrame::new(ExtendedId::new(raw_id).unwrap(), &data) {
        if let Err(e) = tx.transmit_async(&frame).await {
            log::warn!("tx error (msg_type={}): {:?}", msg_type, e);
        }
    }
}
