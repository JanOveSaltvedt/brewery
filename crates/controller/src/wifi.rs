use embassy_executor::Spawner;
use embassy_net::{tcp::TcpSocket, Runner, StackResources};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::{Receiver, Sender},
};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::rng::Rng;
use esp_radio::wifi::{
    scan::{ScanConfig, ScanTypeConfig},
    sta::StationConfig,
    Config as WifiConfig, ControllerConfig, Interface, WifiController,
};
use minimq::{Buffers, ConfigBuilder, Publication, Session, TopicFilter, Will};
use static_cell::StaticCell;


const WIFI_SSID: &str = env!("BREWTECH_CONTROLLER_CONFIG_WIFI_SSID");
const WIFI_PASSWORD: &str = env!("BREWTECH_CONTROLLER_CONFIG_WIFI_PASSWORD");
const MQTT_HOST: &str = env!("BREWTECH_CONTROLLER_CONFIG_MQTT_HOST");
const MQTT_PORT: u16 = esp_config::esp_config_int!(u16, "BREWTECH_CONTROLLER_CONFIG_MQTT_PORT");
const MQTT_USERNAME: &str = env!("BREWTECH_CONTROLLER_CONFIG_MQTT_USERNAME");
const MQTT_PASSWORD: &str = env!("BREWTECH_CONTROLLER_CONFIG_MQTT_PASSWORD");

// Only attempt to connect when the best visible AP for our SSID is at least
// this strong. Avoids thrashing against an AP that the network will immediately
// kick us from due to BSS load-balancing.
const MIN_CONNECT_RSSI: i8 = -70;

const AVAIL_TOPIC: &str = "brewtech/available";
const TEMP_TOPIC: &str = "brewtech/sensor/0/temperature";
const DENSITY_TOPIC: &str = "brewtech/sensor/0/density";
const CALIBRATE_TOPIC: &str = "brewtech/sensor/0/calibrate";
const CALIBRATION_ACK_TOPIC: &str = "brewtech/sensor/0/calibration_ack";

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

const HA_CALIBRATE_DISCOVERY_TOPIC: &str = "homeassistant/number/brewtech_0_calibrate/config";
const HA_CALIBRATE_DISCOVERY_PAYLOAD: &str = concat!(
    r#"{"name":"Calibrate Density","unique_id":"brewtech_0_calibrate","#,
    r#""command_topic":"brewtech/sensor/0/calibrate","#,
    r#""min":0.9,"max":1.2,"step":0.001,"mode":"box","#,
    r#""unit_of_measurement":"SG","icon":"mdi:scale","#,
    r#""availability_topic":"brewtech/available","#,
    r#""device":{"identifiers":["brewtech_0"],"name":"BrewTools Sensor 0","model":"BrewTools"}}"#
);

const HA_CALIBRATION_ACK_DISCOVERY_TOPIC: &str =
    "homeassistant/sensor/brewtech_0_calibration_ack/config";
const HA_CALIBRATION_ACK_DISCOVERY_PAYLOAD: &str = concat!(
    r#"{"name":"Calibration Status","unique_id":"brewtech_0_calibration_ack","#,
    r#""state_topic":"brewtech/sensor/0/calibration_ack","#,
    r#""icon":"mdi:check-circle","#,
    r#""availability_topic":"brewtech/available","#,
    r#""device":{"identifiers":["brewtech_0"],"name":"BrewTools Sensor 0","model":"BrewTools"}}"#
);

static NET_RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface>) {
    runner.run().await;
}

#[embassy_executor::task]
async fn connection_task(mut controller: WifiController<'static>) {
    // Passive scan: listen for beacons instead of sending probe requests.
    // Android hotspots ignore broadcast probe requests (privacy), so active
    // scan's 20 ms window misses them; 150 ms covers the 100 ms beacon
    // interval with margin.
    let passive = ScanTypeConfig::Passive(esp_hal::time::Duration::from_millis(150));
    let mut attempt: u32 = 0;
    loop {
        // Broad scan: discover all visible APs.
        let broad_aps = match controller
            .scan_async(&ScanConfig::default().with_scan_type(passive))
            .await
        {
            Ok(aps) => aps,
            Err(e) => {
                log::warn!("wifi: scan failed: {:?}, retrying in 5s", e);
                Timer::after(Duration::from_secs(5)).await;
                continue;
            }
        };

        for ap in &broad_aps {
            log::info!(
                "wifi: AP ssid='{}' bssid={:02x?} ch={} rssi={} dBm",
                ap.ssid.as_str(),
                ap.bssid,
                ap.channel,
                ap.signal_strength
            );
        }

        // Find the strongest AP for our SSID.
        let best = broad_aps
            .iter()
            .filter(|ap| ap.ssid.as_str() == WIFI_SSID)
            .max_by_key(|ap| ap.signal_strength);

        let (best_rssi, best_bssid, best_channel) = match best {
            None => {
                log::warn!("wifi: SSID '{}' not found, retrying in 15s", WIFI_SSID);
                Timer::after(Duration::from_secs(15)).await;
                continue;
            }
            Some(ap) => (ap.signal_strength, ap.bssid, ap.channel),
        };
        drop(broad_aps);

        if best_rssi < MIN_CONNECT_RSSI {
            log::warn!(
                "wifi: best RSSI {} dBm < threshold {} dBm, retrying in 15s",
                best_rssi,
                MIN_CONNECT_RSSI
            );
            Timer::after(Duration::from_secs(15)).await;
            continue;
        }

        // Pin to the strongest AP's BSSID so the driver cannot fall back to a
        // weaker AP on a different channel.
        log::info!(
            "wifi: pinning to bssid={:02x?} ch={} rssi={} dBm",
            best_bssid,
            best_channel,
            best_rssi
        );
        if let Err(e) = controller.set_config(&WifiConfig::Station(
            StationConfig::default()
                .with_ssid(WIFI_SSID)
                .with_password(WIFI_PASSWORD.into())
                .with_bssid(best_bssid),
        )) {
            log::warn!("wifi: set_config failed: {:?}, retrying in 5s", e);
            Timer::after(Duration::from_secs(5)).await;
            continue;
        }

        attempt += 1;
        log::info!("wifi: connecting (attempt #{})", attempt);
        match controller.connect_async().await {
            Ok(_) => {
                log::info!("wifi: connected to AP");
                controller.wait_for_disconnect_async().await.ok();
                log::warn!("wifi: disconnected from AP, reconnecting...");
            }
            Err(e) => {
                log::warn!("wifi: connect failed (attempt #{}): {:?}", attempt, e);
                Timer::after(Duration::from_secs(5)).await;
            }
        }
    }
}

#[embassy_executor::task]
pub async fn wifi_task(
    wifi: esp_hal::peripherals::WIFI<'static>,
    spawner: Spawner,
    cmd_sender: Sender<'static, CriticalSectionRawMutex, f32, 1>,
    ack_receiver: Receiver<'static, CriticalSectionRawMutex, u32, 1>,
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

    log::info!(
        "wifi: connecting to SSID '{}' with password '{}'",
        WIFI_SSID,
        WIFI_PASSWORD
    );
    log::info!("wifi: waiting for IP...");
    stack.wait_config_up().await;
    log::info!("wifi: IP acquired");

    let mut mqtt_rx_buf = [0u8; 256];
    let mut mqtt_tx_buf = [0u8; 2048];
    let mut tcp_rx_buf = [0u8; 1024];
    let mut tcp_tx_buf = [0u8; 512];

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
            .keepalive_interval(5);
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

        macro_rules! publish_or_reconnect {
            ($pub:expr, $label:literal) => {
                if $pub.await.is_err() {
                    log::warn!("mqtt: failed to publish {}", $label);
                    continue 'reconnect;
                }
            };
        }

        publish_or_reconnect!(
            session.publish(Publication::new(AVAIL_TOPIC, b"online" as &[u8]).retain()),
            "available"
        );
        publish_or_reconnect!(
            session.publish(
                Publication::new(HA_TEMP_DISCOVERY_TOPIC, HA_TEMP_DISCOVERY_PAYLOAD.as_bytes())
                    .retain()
                    .qos(minimq::QoS::AtLeastOnce),
            ),
            "temp discovery"
        );
        publish_or_reconnect!(
            session.publish(
                Publication::new(
                    HA_DENSITY_DISCOVERY_TOPIC,
                    HA_DENSITY_DISCOVERY_PAYLOAD.as_bytes(),
                )
                .retain()
                .qos(minimq::QoS::AtLeastOnce),
            ),
            "density discovery"
        );
        publish_or_reconnect!(
            session.publish(
                Publication::new(
                    HA_CALIBRATE_DISCOVERY_TOPIC,
                    HA_CALIBRATE_DISCOVERY_PAYLOAD.as_bytes(),
                )
                .retain()
                .qos(minimq::QoS::AtLeastOnce),
            ),
            "calibrate discovery"
        );
        publish_or_reconnect!(
            session.publish(
                Publication::new(
                    HA_CALIBRATION_ACK_DISCOVERY_TOPIC,
                    HA_CALIBRATION_ACK_DISCOVERY_PAYLOAD.as_bytes(),
                )
                .retain()
                .qos(minimq::QoS::AtLeastOnce),
            ),
            "calibration_ack discovery"
        );

        if let Err(e) = session
            .subscribe(&[TopicFilter::new(CALIBRATE_TOPIC)], &[])
            .await
        {
            log::warn!("mqtt: subscribe failed: {:?}", e);
            continue 'reconnect;
        }

        log::info!("mqtt: discovery published");

        // Reset on each (re)connect so we publish immediately after reconnecting.
        let mut last_temp_publish: Option<Instant> = None;

        loop {
            // Latest temperature — rate-limited to once per 30 s.
            if let Some(celsius) = crate::TEMP_SIGNAL.try_take() {
                let now = Instant::now();
                if last_temp_publish
                    .map(|t| now - t >= Duration::from_secs(30))
                    .unwrap_or(true)
                {
                    let mut buf = [0u8; 32];
                    if let Some(s) = format_float(celsius, 2, &mut buf) {
                        log::info!("Publishing temperature as {}C", s);
                        if session
                            .publish(Publication::new(TEMP_TOPIC, s.as_bytes()))
                            .await
                            .is_err()
                        {
                            Timer::after(Duration::from_secs(5)).await;
                            continue 'reconnect;
                        }
                    }
                    last_temp_publish = Some(now);
                }
            }

            // Latest density — publish immediately on every new reading.
            if let Some(sg) = crate::DENSITY_SIGNAL.try_take() {
                let mut buf = [0u8; 32];
                if let Some(s) = format_float(sg, 4, &mut buf) {
                    log::info!("Publishing SG as {}", s);
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

            // Publish any calibration ACKs received from the sensor.
            while let Ok(ack) = ack_receiver.try_receive() {
                let status = ack_type_str(ack);
                log::info!("mqtt: calibration ack = {}", status);
                if session
                    .publish(Publication::new(CALIBRATION_ACK_TOPIC, status.as_bytes()).retain())
                    .await
                    .is_err()
                {
                    Timer::after(Duration::from_secs(5)).await;
                    continue 'reconnect;
                }
            }

            match embassy_time::with_timeout(Duration::from_millis(100), session.poll()).await {
                Ok(Ok(Some(msg))) => {
                    if msg.topic() == CALIBRATE_TOPIC {
                        if let Ok(s) = core::str::from_utf8(msg.payload()) {
                            if let Ok(sg) = s.trim().parse::<f32>() {
                                log::info!("mqtt: calibrate cmd sg={:.4}", sg);
                                cmd_sender.try_send(sg).ok();
                            }
                        }
                    }
                }
                Ok(Ok(None)) => {}
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

fn ack_type_str(ack: u32) -> &'static str {
    match ack {
        1 => "calibrating",
        2 => "ok",
        3 => "error",
        _ => "none",
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
