use std::{
    thread,
    sync::mpsc,
    time::{Duration, SystemTime}
};
use esp_idf_svc::{
    eventloop::{EspEventLoop, System, EspSystemEventLoop},
    hal::{
        adc::{attenuation, AdcChannelDriver, AdcDriver, config::Config},
        peripherals::Peripherals, modem, peripheral::Peripheral
    },
    nvs::{EspDefaultNvsPartition, EspNvsPartition, NvsDefault},
    wifi::{Configuration, EspWifi, ClientConfiguration, AuthMethod, BlockingWifi},
    mqtt::client::{EspMqttClient, MqttClientConfiguration, EspMqttConnection, QoS,
            EventPayload::{Connected, Published, Received, Subscribed}
    },
    sys::EspError
};
use esp_idf_svc::hal::adc::ADC1;
use esp_idf_svc::hal::gpio::Gpio34;
use log::*;

// WiFi
const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PASSWORD: &str = env!("WIFI_PASSWORD");

// MQTT
const MQTT_BROKER: &str = env!("MQTT_BROKER");
const MQTT_COMMAND_TOPIC: &str = env!("MQTT_COMMAND_TOPIC");
const MQTT_RESPONSE_TOPIC: &str = env!("MQTT_RESPONSE_TOPIC");
const MQTT_CLIENT_ID: &str = "ESP32";

// Values used for the temperature calculation
const T_1: f32 = 0.0;       // Min temp
const T_2: f32 = 50.0;      // Max temp
const V_1: f32 = 2100.0;    // Voltage at max temp
const V_2: f32 = 1558.0;    // Voltage at min temp

const V_T: f32 = (V_2 - V_1) / (T_2 - T_1); // Constant value based on the min and max

fn calc_temp(voltage: f32) -> f32 {
    ((voltage - V_1) / V_T) + T_1
}

fn main() {
    //============================================================================================//
    // PHASE 0 - Initialization                                                                   //
    //============================================================================================//

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

    // Setup ADC1 on pin GPIO34
    let (adc1, pin34) =
        match setup_adc(peripherals.adc1, peripherals.pins.gpio34) {
            Ok((adc1, pin34)) => (adc1, pin34),
            Err(e) => {
                error!("Failed to enable ADC1 for GPIO34\n{e}");
                return
            }
        };

    // Setup WiFi connection
    let _wifi = match setup_wifi(peripherals.modem, event_loop, nvs) {
        Ok(wifi) => wifi,
        Err(e) => {
            error!("Please check Wi-Fi ssid and password are correct\n{e}");
            return
        }
    };

    // Setup MQTT connection
    let (mqtt_client, mqtt_conn) = match setup_mqtt() {
        Ok((client, conn)) => (client, conn),
        Err(e) => {
            error!("Please check address to MQTT is correct\n{e}");
            return
        }
    };

    // Run and handle MQTT subscriptions and publications
    handle_mqtt(start_time, adc1, pin34, mqtt_client, mqtt_conn);
}

fn setup_adc(
    adc_driver: ADC1,
    pin: impl Peripheral<P=Gpio34> + Sized + 'static
) -> Result<(
    AdcDriver<'static, ADC1>, AdcChannelDriver<'static, { attenuation::DB_11 }, Gpio34>
), EspError> {
    let adc1 = AdcDriver::new(adc_driver, &Config::new().calibration(true))?;
    let pin34: AdcChannelDriver<{ attenuation::DB_11 }, _> = AdcChannelDriver::new(pin)?;
    Ok((adc1, pin34))
}

fn setup_wifi(
    modem: impl Peripheral<P = modem::Modem> + 'static,
    event_loop: EspEventLoop<System>,
    nvs: EspNvsPartition<NvsDefault>
) -> Result<BlockingWifi<EspWifi<'static>>, EspError> {
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(modem, event_loop.clone(), Some(nvs)).unwrap(),
        event_loop,
    )?;

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: WIFI_SSID.try_into().unwrap(),
        password: WIFI_PASSWORD.try_into().unwrap(),
        auth_method: AuthMethod::None,
        ..Default::default()
    }))?;

    wifi.start()?;
    wifi.connect()?;
    wifi.wait_netif_up()?;
    info!("Connected to WiFi");
    Ok(wifi)
}

fn setup_mqtt() -> Result<(EspMqttClient<'static>, EspMqttConnection), EspError> {
    let mqtt_cfg = MqttClientConfiguration {
        client_id: Some(MQTT_CLIENT_ID),
        ..Default::default()
    };

    let (mqtt_client, mqtt_conn) =
        EspMqttClient::new(MQTT_BROKER, &mqtt_cfg)?;
    info!("MQTT Connected");
    Ok((mqtt_client, mqtt_conn))
}

fn handle_mqtt(
    start_time: SystemTime,
    mut adc1: AdcDriver<ADC1>,
    mut pin34: AdcChannelDriver<{ attenuation::DB_11 }, Gpio34>,
    mut mqtt_client: EspMqttClient,
    mut mqtt_conn: EspMqttConnection
) {
    // Channel for sending event commands out of the MQTT thread
    let (tx, rx) = mpsc::channel::<String>();

    // Thread for handling different MQTT events
    thread::spawn(move || {
        info!("MQTT Listening for messages");
        while let Ok(event) = mqtt_conn.next() {
            match event.payload() {
                Connected(_) => { info!("Connected"); },
                Subscribed(id) => { info!("Subscribed id {}", id); },
                Published(id) => { info!("Published id {}", id); },
                //================================================================================//
                // PHASE 2 - Command Reception                                                    //
                //================================================================================//
                Received { data, .. } => {
                    if data != [] {
                        let msg = std::str::from_utf8(data).unwrap();
                        info!("Received data: {}", msg);
                        tx.send(msg.to_owned()).unwrap(); // Send data over channel
                    }
                }
                _ => error!("{:?}", event.payload())
            };
        }
        info!("MQTT connection loop exit");
    }); // MQTT event thread

    //============================================================================================//
    // PHASE 1 - Subscription                                                                     //
    //============================================================================================//

    mqtt_client.subscribe(MQTT_COMMAND_TOPIC, QoS::ExactlyOnce).unwrap();

    //============================================================================================//
    // PHASE 3 - Response                                                                         //
    //============================================================================================//

    // Handle the different command from the MQTT event thread
    for x in rx { // Receive data from channel
        let command_arr = x.split(":").collect::<Vec<&str>>();
        if command_arr.is_empty() {
            error!("Invalid command string {:?}",x);
            continue;
        }
        match command_arr[0] {
            "measure" =>
                handle_measure(start_time, &mut adc1, &mut pin34, &mut mqtt_client, &command_arr),
            _ => error!("Unknown command {:?}", command_arr[0])
        };
    } // Command handler
}

fn handle_measure(
    start_time: SystemTime,
    adc1: &mut AdcDriver<ADC1>,
    mut pin34: &mut AdcChannelDriver<{ attenuation::DB_11 }, Gpio34>,
    mqtt_client: &mut EspMqttClient,
    command_arr: &Vec<&str>
) {
    if command_arr.len() < 2 {
        error!("Missing args in command 'measure'");
        return;
    }
    let (amount, delay) = match parse_measure_args(command_arr[1]) {
        Some(value) => value,
        None => return,
    };
    for i in (0..amount).rev() { // From amount to 0
        thread::sleep(Duration::from_millis(delay));
        mqtt_client.publish(
            MQTT_RESPONSE_TOPIC,
            QoS::ExactlyOnce,
            false,
            format!("{},{:.2},{}",
                    i, // Remaining amount
                    calc_temp(adc1.read(&mut pin34).unwrap() as f32), // Temperature
                    start_time.elapsed().unwrap().as_millis() // Device uptime
            ).as_bytes()
        ).unwrap();
    }
}

fn parse_measure_args(arg_string: &str) -> Option<(u64, u64)> {
    let args = arg_string.split(",").collect::<Vec<&str>>();
    if args.len() != 2 {
        error!("Wrong args amount on 'measure', expected 2, got {}", args.len());
        return None;
    }
    let amount: u64 = match args[0].parse::<u64>() {
        Ok(num) => num,
        Err(e) => {
            error!("Failed to parse amount arg (measure:->here<-,delay), {e}");
            return None;
        }
    };
    let delay: u64 = match args[1].parse::<u64>() {
        Ok(num) => num,
        Err(e) => {
            error!("Failed to parse delay arg (measure:amount,->here<-), {e}");
            return None;
        }
    };
    Some((amount, delay))
}