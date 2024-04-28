use std::{
    thread,
    sync::mpsc,
    time::{Duration, SystemTime}
};
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    nvs::{EspDefaultNvsPartition, EspNvsPartition, NvsDefault},
    wifi::Configuration,
    wifi::EspWifi,
    wifi::ClientConfiguration,
    wifi::AuthMethod,
    hal::{
        peripherals::Peripherals,
        adc::attenuation::DB_11,
        adc::oneshot::{AdcChannelDriver, AdcDriver},
        adc::oneshot::config::AdcChannelConfig,
        modem,
        peripheral::Peripheral
    },
    eventloop::{EspEventLoop, System},
    mqtt::client::{EspMqttClient, MqttClientConfiguration, QoS},
    mqtt::client::EventPayload::{Connected, Published, Received, Subscribed},
    wifi::BlockingWifi
};
use esp_idf_svc::mqtt::client::EspMqttConnection;
use log::*;


// WiFi
const WIFI_SSID: &str = "";
const WIFI_PASSWORD: &str = "";

// MQTT
const MQTT_URL: &str = "mqtt://192.168.1.216:1883";
const MQTT_SUB_TOPIC: &str = "iot/assignment2/topics/subscribe";
const MQTT_PUB_TOPIC: &str = "iot/assignment2/topics/publish";
const MQTT_CLIENT_ID: &str = "ESP32";

fn main() {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_svc::sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    // Time stamp for device running time
    let start_time = SystemTime::now();

    let peripherals = Peripherals::take().unwrap();
    let event_loop = EspSystemEventLoop::take().unwrap();
    let nvs = EspDefaultNvsPartition::take().unwrap();

    // Setup WiFi connection
    let _wifi = setup_wifi(peripherals.modem, event_loop, nvs).unwrap();

    // Setup MQTT connection
    let (mut mqtt_client, mut mqtt_conn) = setup_mqtt();

    // Channel for sending event commands out of the MQTT thread
    let (tx, rx) = mpsc::channel::<String>();

    // Setup for the ADC1 on pin GPIO34
    let adc_config = AdcChannelConfig {
        attenuation: DB_11,
        calibration: true,
        ..Default::default()
    };
    let adc = AdcDriver::new(peripherals.adc1).unwrap();
    let mut adc_pin=
        AdcChannelDriver::new(&adc, peripherals.pins.gpio34, &adc_config).unwrap();

    // Thread for handling different MQTT events
    thread::spawn(move || {
        info!("MQTT Listening for messages");
        while let Ok(event) = mqtt_conn.next() {
            match event.payload() {
                Connected(_) => { info!("Connected"); },
                Subscribed(id) => { info!("Subscribed id {}", id); },
                Published(id) => { info!("Published id {}", id); },
                Received{data, ..} => {
                    if data != [] {
                        let msg = std::str::from_utf8(data).unwrap();
                        info!("Received data: {}", msg);
                        tx.send(msg.to_owned()).unwrap(); // Send data over channel
                    }
                }
                _ => {
                    error!("{:?}", event.payload());
                }
            };
        }
        info!("MQTT connection loop exit");
    }); // MQTT event thread

    mqtt_client.subscribe(MQTT_SUB_TOPIC, QoS::ExactlyOnce).unwrap();

    // Handle the different command from the MQTT event thread
    for x in rx { // Receive data from channel
        let command_arr = x.split(":").collect::<Vec<&str>>();
        if command_arr.is_empty() {
            error!("Invalid command string {:?}",x);
            continue;
        }
        match command_arr[0] {
            "measure" => {
                let args = command_arr[1].split(",").collect::<Vec<&str>>();
                if args.len() != 2 {
                    error!("Wrong args amount on 'measure', expected 2, got {}", args.len());
                    continue;
                }
                let amount: u64 = match args[0].parse::<u64>() {
                    Ok(num) => num,
                    Err(e) => {
                        error!("Failed to parse amount arg (measure:->here<-,delay), {e}");
                        continue;
                    }
                };
                let delay: u64 = match args[1].parse::<u64>() {
                    Ok(num) => num,
                    Err(e) => {
                        error!("Failed to parse delay arg (measure:amount,->here<-), {e}");
                        continue;
                    }
                };
                for i in (0..amount).rev() { // From amount to 0
                    thread::sleep(Duration::from_millis(delay));
                    mqtt_client.publish(
                        MQTT_PUB_TOPIC,
                        QoS::ExactlyOnce,
                        false,
                        format!("{},{:.2},{}",
                                i, // Remaining amount
                                calc_temp(adc.read(&mut adc_pin).unwrap() as f32), // Temperature
                                start_time.elapsed().unwrap().as_millis() // Device uptime
                        ).as_bytes()
                    ).unwrap();
                }
            },
            _ => {
                error!("Unknown command {:?}", command_arr[0]);
            }
        };
    } // Command handler

    loop {
        thread::sleep(Duration::from_millis(1000));
    }

}

// Values used for the temperature calculation
const T_1: f32 = 0.0;       // Min temp
const T_2: f32 = 50.0;      // Max temp
const V_1: f32 = 2100.0;    // Voltage at max temp
const V_2: f32 = 1558.0;    // Voltage at min temp

const V_T: f32 = (V_2 - V_1) / (T_2 - T_1); // Constant value based on the min and max

fn calc_temp(voltage: f32) -> f32 {
    ((voltage - V_1) / V_T) + T_1
}

fn setup_wifi(
    modem: impl Peripheral<P = modem::Modem> + 'static,
    event_loop: EspEventLoop<System>,
    nvs: EspNvsPartition<NvsDefault>
) -> Option<BlockingWifi<EspWifi<'static>>> {
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(modem, event_loop.clone(), Some(nvs)).unwrap(),
        event_loop,
    ).unwrap();

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: WIFI_SSID.try_into().unwrap(),
        password: WIFI_PASSWORD.try_into().unwrap(),
        auth_method: AuthMethod::None,
        ..Default::default()
    })).unwrap();

    wifi.start().unwrap();

    wifi.connect().unwrap();

    wifi.wait_netif_up().unwrap();

    info!("Connected to WiFi");

    Some(wifi)
}

fn setup_mqtt() -> (EspMqttClient<'static>, EspMqttConnection) {
    let mqtt_cfg = MqttClientConfiguration {
        client_id: Some(MQTT_CLIENT_ID),
        ..Default::default()
    };

    let (mqtt_client, mqtt_conn) =
        EspMqttClient::new(MQTT_URL, &mqtt_cfg).unwrap();
    info!("MQTT Connected");
    (mqtt_client, mqtt_conn)
}