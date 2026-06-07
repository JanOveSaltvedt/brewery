use embassy_executor::Spawner;
use embassy_net::{Runner, StackResources, tcp::TcpSocket};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::{Duration, Timer};
use esp_hal::rng::Rng;
use esp_radio::wifi::{
    Config as WifiConfig, ControllerConfig, Interface, WifiController,
    sta::StationConfig,
};
use minimq::{Buffers, ConfigBuilder, Publication, Session, Will};
use static_cell::StaticCell;

use crate::SensorState;

const WIFI_SSID: &str = env!("BREWTECH_CONTROLLER_CONFIG_WIFI_SSID");
const WIFI_PASSWORD: &str = env!("BREWTECH_CONTROLLER_CONFIG_WIFI_PASSWORD");
const MQTT_HOST: &str = env!("BREWTECH_CONTROLLER_CONFIG_MQTT_HOST");
const MQTT_PORT: u16 =
    esp_config::esp_config_int!(u16, "BREWTECH_CONTROLLER_CONFIG_MQTT_PORT");
const MQTT_USERNAME: &str = env!("BREWTECH_CONTROLLER_CONFIG_MQTT_USERNAME");
const MQTT_PASSWORD: &str = env!("BREWTECH_CONTROLLER_CONFIG_MQTT_PASSWORD");

const AVAIL_TOPIC: &str = "brewtech/available";
const TEMP_TOPIC: &str = "brewtech/sensor/0/temperature";
const DENSITY_TOPIC: &str = "brewtech/sensor/0/density";

const HA_TEMP_DISCOVERY_TOPIC: &str = "homeassistant/sensor/brewtech_0_temperature/config";
const HA_TEMP_DISCOVERY_PAYLOAD: &str = concat!(
    r#"{"name":"Temperature","unique_id":"brewtech_0_temperature","#,
    r#""state_topic":"brewtech/sensor/0/temperature","#,
    r#""device_class":"temperature","unit_of_measurement":"°C","#,
    r#""availability_topic":"brewtech/available","#,
    r#""device":{"identifiers":["brewtech_0"],"name":"BrewTools Sensor 0","model":"BrewTools"}}"#
);

const HA_DENSITY_DISCOVERY_TOPIC: &str = "homeassistant/sensor/brewtech_0_density/config";
const HA_DENSITY_DISCOVERY_PAYLOAD: &str = concat!(
    r#"{"name":"Specific Gravity","unique_id":"brewtech_0_density","#,
    r#""state_topic":"brewtech/sensor/0/density","#,
    r#""unit_of_measurement":"SG","icon":"mdi:beer","#,
    r#""availability_topic":"brewtech/available","#,
    r#""device":{"identifiers":["brewtech_0"],"name":"BrewTools Sensor 0","model":"BrewTools"}}"#
);

#[derive(Clone, Copy, PartialEq)]
struct PublishedState {
    temperature: Option<u32>, // bits of f32, avoids PartialEq issues
    density: Option<u32>,
}

impl PublishedState {
    fn from_sensor(s: &SensorState) -> Self {
        Self {
            temperature: s.temperature.map(f32::to_bits),
            density: s.density.map(f32::to_bits),
        }
    }
}

static NET_RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface>) {
    runner.run().await;
}

#[embassy_executor::task]
async fn connection_task(mut controller: WifiController<'static>) {
    loop {
        match controller.connect_async().await {
            Ok(_) => {
                controller.wait_for_disconnect_async().await.ok();
            }
            Err(_) => {
                Timer::after(Duration::from_secs(5)).await;
            }
        }
    }
}

#[embassy_executor::task]
pub async fn wifi_task(
    wifi: esp_hal::peripherals::WIFI<'static>,
    spawner: Spawner,
    state: &'static Mutex<CriticalSectionRawMutex, SensorState>,
) {
    let station_config = WifiConfig::Station(
        StationConfig::default()
            .with_ssid(WIFI_SSID)
            .with_password(WIFI_PASSWORD.into()),
    );
    let wifi_interface = Interface::station();
    let controller = WifiController::new(
        wifi,
        ControllerConfig::default().with_initial_config(station_config),
    )
    .unwrap();

    let rng = Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;
    let (stack, runner) = embassy_net::new(
        wifi_interface,
        embassy_net::Config::dhcpv4(Default::default()),
        NET_RESOURCES.init(StackResources::<3>::new()),
        seed,
    );

    spawner.spawn(net_task(runner).unwrap());
    spawner.spawn(connection_task(controller).unwrap());

    log::info!("wifi: waiting for IP...");
    stack.wait_config_up().await;
    log::info!("wifi: IP acquired");

    let mut mqtt_rx_buf = [0u8; 256];
    let mut mqtt_tx_buf = [0u8; 768];
    let mut tcp_rx_buf = [0u8; 1024];
    let mut tcp_tx_buf = [0u8; 512];

    let mut last_published = PublishedState::from_sensor(&*state.lock().await);

    'reconnect: loop {
        let Ok(broker_ip) = parse_ipv4(MQTT_HOST) else {
            log::error!("mqtt: invalid host '{}'", MQTT_HOST);
            Timer::after(Duration::from_secs(30)).await;
            continue;
        };

        let will = Will::new(AVAIL_TOPIC, b"offline", &[]).unwrap();
        let mut config = ConfigBuilder::new(Buffers::new(&mut mqtt_rx_buf, &mut mqtt_tx_buf))
            .will(will)
            .unwrap()
            .client_id("brewtech")
            .unwrap()
            .keepalive_interval(60);
        if !MQTT_USERNAME.is_empty() {
            config = config
                .auth(MQTT_USERNAME, MQTT_PASSWORD.as_bytes())
                .unwrap();
        }
        let mut session: Session<'_, _> = Session::new(config);

        let mut socket = TcpSocket::new(stack, &mut tcp_rx_buf, &mut tcp_tx_buf);
        socket.set_timeout(Some(Duration::from_secs(15)));

        let addr = embassy_net::IpEndpoint::new(broker_ip.into(), MQTT_PORT);
        if let Err(e) = socket.connect(addr).await {
            log::warn!("mqtt: TCP connect failed: {:?}", e);
            Timer::after(Duration::from_secs(5)).await;
            continue;
        }

        if let Err(e) = session.connect(socket).await {
            log::warn!("mqtt: MQTT connect failed: {:?}", e);
            Timer::after(Duration::from_secs(5)).await;
            continue;
        }
        log::info!("mqtt: connected");

        if session
            .publish(Publication::new(AVAIL_TOPIC, b"online" as &[u8]).retain())
            .await
            .is_err()
        {
            continue 'reconnect;
        }

        if session
            .publish(
                Publication::new(HA_TEMP_DISCOVERY_TOPIC, HA_TEMP_DISCOVERY_PAYLOAD.as_bytes())
                    .retain()
                    .qos(minimq::QoS::AtLeastOnce),
            )
            .await
            .is_err()
        {
            continue 'reconnect;
        }

        if session
            .publish(
                Publication::new(
                    HA_DENSITY_DISCOVERY_TOPIC,
                    HA_DENSITY_DISCOVERY_PAYLOAD.as_bytes(),
                )
                .retain()
                .qos(minimq::QoS::AtLeastOnce),
            )
            .await
            .is_err()
        {
            continue 'reconnect;
        }

        log::info!("mqtt: discovery published");

        loop {
            let snap = PublishedState::from_sensor(&*state.lock().await);
            if snap != last_published {
                if snap.temperature != last_published.temperature {
                    if let Some(bits) = snap.temperature {
                        let val = f32::from_bits(bits);
                        let mut buf = [0u8; 32];
                        if let Some(s) = format_float(val, 2, &mut buf) {
                            if session
                                .publish(Publication::new(TEMP_TOPIC, s.as_bytes()))
                                .await
                                .is_err()
                            {
                                Timer::after(Duration::from_secs(5)).await;
                                continue 'reconnect;
                            }
                        }
                    }
                }
                if snap.density != last_published.density {
                    if let Some(bits) = snap.density {
                        let val = f32::from_bits(bits);
                        let mut buf = [0u8; 32];
                        if let Some(s) = format_float(val, 4, &mut buf) {
                            if session
                                .publish(Publication::new(DENSITY_TOPIC, s.as_bytes()))
                                .await
                                .is_err()
                            {
                                Timer::after(Duration::from_secs(5)).await;
                                continue 'reconnect;
                            }
                        }
                    }
                }
                last_published = snap;
            }

            match embassy_time::with_timeout(Duration::from_millis(100), session.poll()).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    log::warn!("mqtt: poll error: {:?}", e);
                    Timer::after(Duration::from_secs(5)).await;
                    continue 'reconnect;
                }
                Err(_timeout) => {}
            }
        }
    }
}

fn parse_ipv4(s: &str) -> Result<embassy_net::Ipv4Address, ()> {
    let mut it = s.split('.');
    let a: u8 = it.next().and_then(|s| s.parse().ok()).ok_or(())?;
    let b: u8 = it.next().and_then(|s| s.parse().ok()).ok_or(())?;
    let c: u8 = it.next().and_then(|s| s.parse().ok()).ok_or(())?;
    let d: u8 = it.next().and_then(|s| s.parse().ok()).ok_or(())?;
    if it.next().is_some() {
        return Err(());
    }
    Ok(embassy_net::Ipv4Address::new(a, b, c, d))
}

/// Formats a float into `buf` with `decimals` fractional digits.
/// Returns a `&str` into `buf` on success.
fn format_float<'a>(val: f32, decimals: u32, buf: &'a mut [u8]) -> Option<&'a str> {
    use core::fmt::Write;
    struct W<'a> {
        buf: &'a mut [u8],
        pos: usize,
    }
    impl Write for W<'_> {
        fn write_str(&mut self, src: &str) -> core::fmt::Result {
            let dst = self
                .buf
                .get_mut(self.pos..)
                .filter(|d| d.len() >= src.len())
                .ok_or(core::fmt::Error)?;
            dst[..src.len()].copy_from_slice(src.as_bytes());
            self.pos += src.len();
            Ok(())
        }
    }
    let mut w = W { buf, pos: 0 };
    match decimals {
        2 => write!(w, "{:.2}", val).ok()?,
        3 => write!(w, "{:.3}", val).ok()?,
        4 => write!(w, "{:.4}", val).ok()?,
        _ => write!(w, "{}", val).ok()?,
    }
    let len = w.pos;
    core::str::from_utf8(&buf[..len]).ok()
}
