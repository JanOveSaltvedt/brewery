#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

mod can_tasks;

use can_tasks::{can_rx_task, density_probe_task, sensor_log_task};
use embassy_executor::Spawner;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
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

    spawner.spawn(can_rx_task(rx, READINGS.sender()).unwrap());
    spawner.spawn(density_probe_task(tx).unwrap());
    spawner.spawn(sensor_log_task(READINGS.receiver()).unwrap());

    log::info!("controller ready — listening for density sensors");
}
