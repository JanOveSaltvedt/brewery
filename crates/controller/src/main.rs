#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

mod can_tasks;
mod wifi;

use can_tasks::{can_rx_task, can_tx_task, sensor_log_task};

const SENSOR_NODE_ID: u8 =
    esp_config::esp_config_int!(u8, "BREWTECH_CONTROLLER_CONFIG_SENSOR_NODE_ID");
use embassy_executor::Spawner;
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::Channel,
    signal::Signal,
};
use esp_hal::{
    interrupt::software::SoftwareInterruptControl,
    timer::timg::TimerGroup,
    twai::{BaudRate, TwaiConfiguration, TwaiMode},
};
use log::LevelFilter;

use esp_backtrace as _;

esp_bootloader_esp_idf::esp_app_desc!();

// ---------------------------------------------------------------------------
// Shared sensor reading type published by can_rx_task, consumed by sensor_log_task.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub enum SensorReading {
    Temperature { node_id: u8, celsius: f32 },
    Density { node_id: u8, sg: f32 },
}

static READINGS: Channel<CriticalSectionRawMutex, SensorReading, 16> = Channel::new();

// Calibration command (wifi_task → can_tx_task): SG float to send to sensor.
static CALIBRATION_CMD: Channel<CriticalSectionRawMutex, f32, 1> = Channel::new();
// Calibration ACK (can_rx_task → wifi_task): decoded ACK type from sensor.
static CALIBRATION_ACK: Channel<CriticalSectionRawMutex, u32, 1> = Channel::new();

// Latest sensor values (node 0 only) — signaled by sensor_log_task, consumed by wifi_task.
// Signal overwrites any unread value, so wifi_task always sees the latest reading.
pub static TEMP_SIGNAL: Signal<CriticalSectionRawMutex, f32> = Signal::new();
pub static DENSITY_SIGNAL: Signal<CriticalSectionRawMutex, f32> = Signal::new();

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[esp_hal::main]
async fn main(spawner: Spawner) {
    esp_println::logger::init_logger(LevelFilter::Info);

    let p = esp_hal::init(esp_hal::Config::default());

    esp_alloc::heap_allocator!(size: 72 * 1024);

    let sw_int = SoftwareInterruptControl::new(p.SW_INTERRUPT);
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    // GPIO0 = CRX (transceiver RX → MCU), GPIO1 = CTX (MCU TX → transceiver).
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

    spawner.spawn(can_rx_task(rx, READINGS.sender(), CALIBRATION_ACK.sender(), SENSOR_NODE_ID).unwrap());
    spawner.spawn(can_tx_task(tx, CALIBRATION_CMD.receiver(), SENSOR_NODE_ID).unwrap());
    spawner.spawn(sensor_log_task(READINGS.receiver(), SENSOR_NODE_ID).unwrap());
    spawner.spawn(wifi::wifi_task(p.WIFI, spawner, CALIBRATION_CMD.sender(), CALIBRATION_ACK.receiver()).unwrap());

    log::info!("controller ready — listening for density sensors");
}
