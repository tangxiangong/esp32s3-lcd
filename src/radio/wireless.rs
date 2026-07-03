use alloc::{
    boxed::Box,
    string::{String, ToString},
    vec::Vec,
};
use core::{
    future::Future,
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};

use bleps::{
    Ble, HciConnector, PollResult,
    ad_structure::{
        AdStructure, BR_EDR_NOT_SUPPORTED, LE_GENERAL_DISCOVERABLE, create_advertising_data,
    },
    att::Uuid,
    attribute::{ATT_READABLE, ATT_WRITEABLE, Attribute},
    attribute_server::{CHARACTERISTIC_UUID16, PRIMARY_SERVICE_UUID16, WorkResult},
    event::EventType,
    no_rng::NoRng,
};
use defmt::{error, info, warn};
use esp_hal::{
    peripherals,
    time::{Duration, Instant},
};
use esp_radio::{
    ble::controller::BleConnector,
    wifi::{
        self, AuthenticationMethod, Config, WifiController, ap::AccessPointInfo, scan::ScanConfig,
        sta::StationConfig,
    },
};

const BLE_DEVICE_NAME: &str = "ESP32S3 LCD";
const BLE_SERVICE_UUID: [u8; 16] = [
    0xe7, 0x3d, 0x9a, 0x10, 0x31, 0x84, 0x46, 0x5f, 0x99, 0x64, 0x87, 0x29, 0x35, 0x6f, 0x2d, 0x21,
];
const BLE_WRITE_UUID: [u8; 16] = [
    0xe7, 0x3d, 0x9a, 0x11, 0x31, 0x84, 0x46, 0x5f, 0x99, 0x64, 0x87, 0x29, 0x35, 0x6f, 0x2d, 0x21,
];
const BLE_READ_UUID: [u8; 16] = [
    0xe7, 0x3d, 0x9a, 0x12, 0x31, 0x84, 0x46, 0x5f, 0x99, 0x64, 0x87, 0x29, 0x35, 0x6f, 0x2d, 0x21,
];
const BLE_READ_VALUE: &[u8] = b"ESP32S3 LCD peripheral ready";
const WIFI_SCAN_LIMIT: usize = 20;

pub struct Wireless {
    wifi: Option<WifiRadio>,
    ble: Option<BleRadio>,
}

struct WifiRadio {
    controller: WifiController<'static>,
    station_mac: [u8; 6],
    access_point_mac: [u8; 6],
    scan_results: Vec<WifiNetwork>,
}

struct BleRadio {
    ble: Ble<'static>,
    connected: bool,
    received_data: [u8; 64],
    received_len: usize,
}

#[derive(Clone, Debug)]
pub struct WifiNetwork {
    pub ssid: String,
    pub bssid: [u8; 6],
    pub channel: u8,
    pub signal_strength: i8,
    pub auth_method: WifiAuthMethod,
}

#[derive(Clone, Debug)]
pub struct WifiStatus {
    pub initialized: bool,
    pub connected: bool,
    pub station_mac: Option<[u8; 6]>,
    pub access_point_mac: Option<[u8; 6]>,
    pub ssid: Option<String>,
    pub bssid: Option<[u8; 6]>,
    pub channel: Option<u8>,
    pub rssi: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, defmt::Format)]
pub enum WifiAuthMethod {
    Open,
    Wep,
    Wpa,
    Wpa2Personal,
    WpaWpa2Personal,
    Wpa2Enterprise,
    Wpa3Personal,
    Wpa2Wpa3Personal,
    Other,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, defmt::Format)]
pub enum WifiActionError {
    NotInitialized,
    InvalidInput,
    Configure,
    Scan,
    Connect,
    Disconnect,
}

impl Wireless {
    pub fn new(wifi: peripherals::WIFI<'static>, bt: peripherals::BT<'static>) -> Self {
        // Wi-Fi/BLE 任一初始化失败都不阻止系统启动；显示和 RTC 功能应继续可用。
        let wifi = match WifiRadio::new(wifi) {
            Ok(radio) => Some(radio),
            Err(error) => {
                error!("wifi init failed: {:?}", error);
                None
            }
        };

        let ble = match BleRadio::new(bt) {
            Ok(radio) => Some(radio),
            Err(error) => {
                error!("ble init failed: {:?}", error);
                None
            }
        };

        Self { wifi, ble }
    }

    pub fn poll(&mut self) {
        // 当前主循环只需要持续轮询 BLE 事件；Wi-Fi 操作由显式 API 触发。
        if let Some(ble) = &mut self.ble {
            ble.poll();
        }
    }

    pub fn scan_networks(&mut self) -> Result<&[WifiNetwork], WifiActionError> {
        self.scan_networks_cooperative(|| {})
    }

    pub fn scan_networks_cooperative<F>(
        &mut self,
        mut on_wait: F,
    ) -> Result<&[WifiNetwork], WifiActionError>
    where
        F: FnMut(),
    {
        let Self { wifi, ble } = self;
        let wifi = wifi.as_mut().ok_or(WifiActionError::NotInitialized)?;
        wifi.scan_networks(|| {
            if let Some(ble) = ble {
                ble.poll();
            }
            on_wait();
        })
    }

    pub fn latest_scan(&self) -> &[WifiNetwork] {
        self.wifi
            .as_ref()
            .map(WifiRadio::latest_scan)
            .unwrap_or_default()
    }

    pub fn connect_station(
        &mut self,
        ssid: &str,
        password: &str,
    ) -> Result<WifiStatus, WifiActionError> {
        self.connect_station_cooperative(ssid, password, || {})
    }

    pub fn connect_station_cooperative<F>(
        &mut self,
        ssid: &str,
        password: &str,
        mut on_wait: F,
    ) -> Result<WifiStatus, WifiActionError>
    where
        F: FnMut(),
    {
        let Self { wifi, ble } = self;
        let wifi = wifi.as_mut().ok_or(WifiActionError::NotInitialized)?;
        wifi.connect_station(ssid, password, || {
            if let Some(ble) = ble {
                ble.poll();
            }
            on_wait();
        })
    }

    pub fn disconnect_station(&mut self) -> Result<WifiStatus, WifiActionError> {
        self.wifi
            .as_mut()
            .ok_or(WifiActionError::NotInitialized)?
            .disconnect_station()
    }

    pub fn wifi_status(&self) -> WifiStatus {
        self.wifi
            .as_ref()
            .map(WifiRadio::status)
            .unwrap_or_else(WifiStatus::not_initialized)
    }

    pub fn wifi_connected(&self) -> bool {
        self.wifi
            .as_ref()
            .map(WifiRadio::is_connected)
            .unwrap_or(false)
    }

    pub fn bluetooth_connected(&self) -> bool {
        self.ble.as_ref().map(BleRadio::connected).unwrap_or(false)
    }

    pub fn bluetooth_last_write_len(&self) -> Option<usize> {
        self.ble.as_ref().map(|ble| ble.last_write().len())
    }

    pub fn restart_bluetooth_advertising(&mut self) -> Result<(), BleActionError> {
        let ble = self.ble.as_mut().ok_or(BleActionError::NotInitialized)?;
        if ble.connected() {
            return Err(BleActionError::Connected);
        }

        ble.enable_advertising()
            .map_err(|_| BleActionError::Advertise)
    }
}

impl WifiRadio {
    fn new(wifi: peripherals::WIFI<'static>) -> Result<Self, WifiInitError> {
        let (controller, interfaces) =
            wifi::new(wifi, Default::default()).map_err(|_| WifiInitError::Controller)?;

        let station_mac = interfaces.station.mac_address();
        let access_point_mac = interfaces.access_point.mac_address();
        info!(
            "wifi station mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            station_mac[0],
            station_mac[1],
            station_mac[2],
            station_mac[3],
            station_mac[4],
            station_mac[5]
        );
        info!(
            "wifi ap mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            access_point_mac[0],
            access_point_mac[1],
            access_point_mac[2],
            access_point_mac[3],
            access_point_mac[4],
            access_point_mac[5]
        );

        Ok(Self {
            controller,
            station_mac,
            access_point_mac,
            scan_results: Vec::new(),
        })
    }

    fn scan_networks<F>(&mut self, on_wait: F) -> Result<&[WifiNetwork], WifiActionError>
    where
        F: FnMut(),
    {
        let config = ScanConfig::default().with_max(WIFI_SCAN_LIMIT);
        let access_points = block_on_with_yield(self.controller.scan_async(&config), on_wait)
            .map_err(|_| WifiActionError::Scan)?;

        self.scan_results.clear();
        self.scan_results
            .extend(access_points.into_iter().map(WifiNetwork::from));

        Ok(&self.scan_results)
    }

    fn latest_scan(&self) -> &[WifiNetwork] {
        &self.scan_results
    }

    fn connect_station(
        &mut self,
        ssid: &str,
        password: &str,
        on_wait: impl FnMut(),
    ) -> Result<WifiStatus, WifiActionError> {
        if ssid.is_empty() || ssid.len() > 32 || password.len() > 64 {
            return Err(WifiActionError::InvalidInput);
        }

        let mut station = StationConfig::default()
            .with_ssid(ssid)
            .with_password(password.into());
        if password.is_empty() {
            station = station.with_auth_method(AuthenticationMethod::None);
        }

        self.controller
            .set_config(&Config::Station(station))
            .map_err(|_| WifiActionError::Configure)?;
        block_on_with_yield(self.controller.connect_async(), on_wait)
            .map_err(|_| WifiActionError::Connect)?;

        Ok(self.status())
    }

    fn disconnect_station(&mut self) -> Result<WifiStatus, WifiActionError> {
        block_on(self.controller.disconnect_async()).map_err(|_| WifiActionError::Disconnect)?;
        Ok(self.status())
    }

    fn status(&self) -> WifiStatus {
        let connected = self.controller.is_connected();
        let ap_info = connected
            .then(|| self.controller.ap_info())
            .and_then(Result::ok);

        WifiStatus {
            initialized: true,
            connected,
            station_mac: Some(self.station_mac),
            access_point_mac: Some(self.access_point_mac),
            ssid: ap_info.as_ref().map(|info| info.ssid.as_str().to_string()),
            bssid: ap_info.as_ref().map(|info| info.bssid),
            channel: ap_info.as_ref().map(|info| info.channel),
            rssi: connected
                .then(|| self.controller.rssi())
                .and_then(Result::ok),
        }
    }

    fn is_connected(&self) -> bool {
        self.controller.is_connected()
    }
}

impl WifiStatus {
    fn not_initialized() -> Self {
        Self {
            initialized: false,
            connected: false,
            station_mac: None,
            access_point_mac: None,
            ssid: None,
            bssid: None,
            channel: None,
            rssi: None,
        }
    }
}

impl From<AccessPointInfo> for WifiNetwork {
    fn from(info: AccessPointInfo) -> Self {
        Self {
            ssid: info.ssid.as_str().to_string(),
            bssid: info.bssid,
            channel: info.channel,
            signal_strength: info.signal_strength,
            auth_method: info.auth_method.into(),
        }
    }
}

impl From<Option<AuthenticationMethod>> for WifiAuthMethod {
    fn from(method: Option<AuthenticationMethod>) -> Self {
        match method {
            Some(AuthenticationMethod::None) => Self::Open,
            Some(AuthenticationMethod::Wep) => Self::Wep,
            Some(AuthenticationMethod::Wpa) => Self::Wpa,
            Some(AuthenticationMethod::Wpa2Personal) => Self::Wpa2Personal,
            Some(AuthenticationMethod::WpaWpa2Personal) => Self::WpaWpa2Personal,
            Some(AuthenticationMethod::Wpa2Enterprise) => Self::Wpa2Enterprise,
            Some(AuthenticationMethod::Wpa3Personal) => Self::Wpa3Personal,
            Some(AuthenticationMethod::Wpa2Wpa3Personal) => Self::Wpa2Wpa3Personal,
            Some(_) => Self::Other,
            None => Self::Unknown,
        }
    }
}

impl BleRadio {
    fn new(bt: peripherals::BT<'static>) -> Result<Self, BleInitError> {
        let connector =
            BleConnector::new(bt, Default::default()).map_err(|_| BleInitError::Controller)?;
        let hci = Box::leak(Box::new(HciConnector::new(connector, millis)));
        let mut ble = Ble::new(hci);

        ble.init().map_err(|_| BleInitError::Host)?;
        let address = ble.cmd_read_br_addr().map_err(|_| BleInitError::Host)?;
        info!(
            "ble address {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            address[5], address[4], address[3], address[2], address[1], address[0]
        );

        // BLE 只作为从机/外设使用：广播本机名称并等待中心设备连接。
        let advertising_data = create_advertising_data(&[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteLocalName(BLE_DEVICE_NAME),
        ])
        .map_err(|_| BleInitError::AdvertisingData)?;

        ble.cmd_set_le_advertising_parameters()
            .map_err(|_| BleInitError::Host)?;
        ble.cmd_set_le_advertising_data(advertising_data)
            .map_err(|_| BleInitError::Host)?;
        let mut radio = Self {
            ble,
            connected: false,
            received_data: [0; 64],
            received_len: 0,
        };
        radio.enable_advertising()?;
        info!("ble advertising as {}", BLE_DEVICE_NAME);

        Ok(radio)
    }

    fn poll(&mut self) {
        if self.connected {
            self.poll_gatt();
            return;
        }

        if let Some(event) = self.ble.poll() {
            match event {
                PollResult::Event(EventType::ConnectionComplete { status, .. }) => {
                    self.connected = status == 0;
                    if self.connected {
                        info!("ble connected");
                    } else {
                        warn!("ble connection failed");
                    }
                }
                PollResult::Event(EventType::DisconnectComplete { .. }) => {
                    self.connected = false;
                    info!("ble disconnected");
                    if let Err(error) = self.enable_advertising() {
                        error!("ble advertising restart failed: {:?}", error);
                    } else {
                        info!("ble advertising restarted");
                    }
                }
                PollResult::Event(_) => warn!("ble controller event"),
                PollResult::AsyncData(_) => warn!("ble async data before connection"),
            }
        }
    }

    fn poll_gatt(&mut self) {
        let mut service_uuid = &BLE_SERVICE_UUID[..];
        let write_char_decl = characteristic_declaration(ATT_WRITEABLE, 3, BLE_WRITE_UUID);
        let read_char_decl = characteristic_declaration(ATT_READABLE, 5, BLE_READ_UUID);
        let mut write_char_decl = &write_char_decl[..];
        let mut read_char_decl = &read_char_decl[..];
        let mut read_value = BLE_READ_VALUE;
        let received_data = self.received_data.as_mut_ptr();
        let received_len = &mut self.received_len as *mut usize;
        let write_handler = move |offset: usize, data: &[u8]| {
            if offset >= 64 {
                return;
            }

            let len = data.len().min(64 - offset);
            // bleps 要求 GATT 表在 AttributeServer 生命周期内看起来是
            // 'static；这里的写回指针只在本次 poll 期间使用。
            unsafe {
                core::ptr::copy_nonoverlapping(data.as_ptr(), received_data.add(offset), len);
                *received_len = offset + len;
            }
            info!("ble write offset {} len {}", offset, len);
        };
        let notify_handler = |_enabled: bool| {};
        let mut write_value = ((), write_handler, notify_handler);

        let mut attributes = [
            Attribute::new(PRIMARY_SERVICE_UUID16, &mut service_uuid),
            Attribute::new(CHARACTERISTIC_UUID16, &mut write_char_decl),
            Attribute::new(Uuid::Uuid128(BLE_WRITE_UUID), &mut write_value),
            Attribute::new(CHARACTERISTIC_UUID16, &mut read_char_decl),
            Attribute::new(Uuid::Uuid128(BLE_READ_UUID), &mut read_value),
        ];
        let mut rng = NoRng;
        let ble = unsafe {
            core::mem::transmute::<&mut Ble<'static>, &'static mut Ble<'static>>(&mut self.ble)
        };
        let attributes = unsafe {
            core::mem::transmute::<&mut [Attribute<'_>], &'static mut [Attribute<'static>]>(
                attributes.as_mut_slice(),
            )
        };
        let rng = unsafe { core::mem::transmute::<&mut NoRng, &'static mut NoRng>(&mut rng) };
        let result = {
            let mut server = bleps::attribute_server::AttributeServer::new(ble, attributes, rng);
            server.do_work()
        };

        match result {
            Ok(WorkResult::DidWork) => {}
            Ok(WorkResult::GotDisconnected) => {
                self.connected = false;
                info!("ble disconnected");
                if let Err(error) = self.enable_advertising() {
                    error!("ble advertising restart failed: {:?}", error);
                } else {
                    info!("ble advertising restarted");
                }
            }
            Err(_) => warn!("ble gatt work failed"),
        }
    }

    fn enable_advertising(&mut self) -> Result<(), BleInitError> {
        self.ble
            .cmd_set_le_advertise_enable(true)
            .map(|_| ())
            .map_err(|_| BleInitError::Host)
    }

    fn connected(&self) -> bool {
        self.connected
    }

    fn last_write(&self) -> &[u8] {
        &self.received_data[..self.received_len]
    }
}

#[derive(Clone, Copy, Debug, defmt::Format)]
enum WifiInitError {
    Controller,
}

#[derive(Clone, Copy, Debug, defmt::Format)]
enum BleInitError {
    Controller,
    Host,
    AdvertisingData,
}

#[derive(Clone, Copy, Debug, defmt::Format)]
pub enum BleActionError {
    NotInitialized,
    Connected,
    Advertise,
}

fn millis() -> u64 {
    Instant::now().duration_since_epoch().as_millis()
}

fn block_on<F: Future>(future: F) -> F::Output {
    block_on_with_yield(future, || {})
}

fn block_on_with_yield<F, Y>(future: F, mut on_wait: Y) -> F::Output
where
    F: Future,
    Y: FnMut(),
{
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);
    let mut future = core::pin::pin!(future);

    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        on_wait();
        esp_rtos::CurrentThreadHandle::get().delay(Duration::from_millis(5));
    }
}

fn characteristic_declaration(properties: u8, value_handle: u16, uuid: [u8; 16]) -> [u8; 19] {
    let mut declaration = [0; 19];
    declaration[0] = properties;
    declaration[1] = (value_handle & 0xff) as u8;
    declaration[2] = (value_handle >> 8) as u8;
    declaration[3..].copy_from_slice(&uuid);
    declaration
}

fn noop_waker() -> Waker {
    // SAFETY: The vtable ignores the data pointer and never dereferences it.
    unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &NOOP_WAKER_VTABLE)) }
}

const NOOP_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
    noop_waker_clone,
    noop_waker_wake,
    noop_waker_wake,
    noop_waker_drop,
);

unsafe fn noop_waker_clone(_: *const ()) -> RawWaker {
    RawWaker::new(core::ptr::null(), &NOOP_WAKER_VTABLE)
}

unsafe fn noop_waker_wake(_: *const ()) {}

unsafe fn noop_waker_drop(_: *const ()) {}
