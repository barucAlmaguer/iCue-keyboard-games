use std::collections::HashMap;
use std::env;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

const PACKET_MAGIC: &[u8; 4] = b"ORGB";
const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 6742;
const CLIENT_PROTOCOL_MAX: u32 = 5;
const DEVICE_TYPE_KEYBOARD: i32 = 5;

const PACKET_ID_REQUEST_CONTROLLER_COUNT: u32 = 0;
const PACKET_ID_REQUEST_CONTROLLER_DATA: u32 = 1;
const PACKET_ID_REQUEST_PROTOCOL_VERSION: u32 = 40;
const PACKET_ID_SET_CLIENT_NAME: u32 = 50;
const PACKET_ID_UPDATE_LEDS: u32 = 1050;
const PACKET_ID_SET_CUSTOM_MODE: u32 = 1100;

#[derive(Clone, Copy)]
struct RgbColor
{
    r: u8,
    g: u8,
    b: u8,
}

pub struct LedColor
{
    pub id: u32,
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

pub struct Keyboard
{
    stream: TcpStream,
    device_idx: u32,
    device_name: String,
    led_map: HashMap<char, u32>,
    led_buffer: Vec<u32>,
}

impl Keyboard
{
    pub fn connect() -> Result<Self, String>
    {
        let addr = openrgb_addr()?;
        let mut stream = TcpStream::connect(&addr)
            .map_err(|err| format!("Failed to connect to OpenRGB at {addr}: {err}"))?;
        stream
            .set_read_timeout(Some(Duration::from_millis(750)))
            .map_err(|err| format!("Failed to set read timeout: {err}"))?;
        stream
            .set_write_timeout(Some(Duration::from_millis(750)))
            .map_err(|err| format!("Failed to set write timeout: {err}"))?;

        send_packet(&mut stream, 0, PACKET_ID_SET_CLIENT_NAME, b"icue-kb-games\0")?;

        let protocol_version = negotiate_protocol(&mut stream)?;
        let controller_count = request_controller_count(&mut stream)?;
        if controller_count == 0 {
            return Err("OpenRGB reports zero controllers. Ensure your keyboard is detected.".to_string());
        }

        let mut devices = Vec::new();
        for idx in 0..controller_count {
            let data = request_controller_data(&mut stream, idx, protocol_version)?;
            devices.push(data);
        }

        let device = select_keyboard(devices)?;
        send_packet(&mut stream, device.idx, PACKET_ID_SET_CUSTOM_MODE, &[])?;

        let led_map = build_led_map(&device.led_names, &device.led_alt_names);
        if led_map.is_empty() {
            return Err("No usable LED names found for this keyboard in OpenRGB.".to_string());
        }

        let led_buffer = vec![0u32; device.led_names.len()];

        Ok(Self {
            stream,
            device_idx: device.idx,
            device_name: device.display_name,
            led_map,
            led_buffer,
        })
    }

    pub fn device_name(&self) -> &str
    {
        &self.device_name
    }

    pub fn led_for_char(&self, ch: char) -> Option<u32>
    {
        let key = ch.to_ascii_uppercase();
        self.led_map.get(&key).copied()
    }

    pub fn set_leds(&mut self, leds: &[LedColor]) -> Result<(), String>
    {
        self.led_buffer.fill(0);
        for led in leds {
            if (led.id as usize) < self.led_buffer.len() {
                let color = RgbColor {
                    r: led.r,
                    g: led.g,
                    b: led.b,
                };
                self.led_buffer[led.id as usize] = rgb_to_u32(color);
            }
        }

        send_update_leds(&mut self.stream, self.device_idx, &self.led_buffer)
    }
}

impl Drop for Keyboard
{
    fn drop(&mut self)
    {
        self.led_buffer.fill(0);
        let _ = send_update_leds(&mut self.stream, self.device_idx, &self.led_buffer);
    }
}

struct DeviceData
{
    idx: u32,
    device_type: i32,
    display_name: String,
    vendor: String,
    led_names: Vec<String>,
    led_alt_names: Vec<String>,
}

fn openrgb_addr() -> Result<String, String>
{
    let host = env::var("OPENRGB_HOST").unwrap_or_else(|_| DEFAULT_HOST.to_string());
    let port = match env::var("OPENRGB_PORT") {
        Ok(value) => value
            .parse::<u16>()
            .map_err(|_| "OPENRGB_PORT must be a valid u16".to_string())?,
        Err(_) => DEFAULT_PORT,
    };

    Ok(format!("{host}:{port}"))
}

fn negotiate_protocol(stream: &mut TcpStream) -> Result<u32, String>
{
    let payload = CLIENT_PROTOCOL_MAX.to_le_bytes();
    send_packet(
        stream,
        0,
        PACKET_ID_REQUEST_PROTOCOL_VERSION,
        &payload,
    )?;

    for _ in 0..3 {
        match try_read_packet(stream)? {
            Some(packet) if packet.packet_id == PACKET_ID_REQUEST_PROTOCOL_VERSION => {
                let mut cursor = Cursor::new(&packet.payload);
                let server_version = cursor.read_u32()?;
                return Ok(server_version.min(CLIENT_PROTOCOL_MAX));
            }
            Some(_) => continue,
            None => break,
        }
    }

    Ok(0)
}

fn request_controller_count(stream: &mut TcpStream) -> Result<u32, String>
{
    send_packet(stream, 0, PACKET_ID_REQUEST_CONTROLLER_COUNT, &[])?;
    let packet = read_packet_expect(stream, PACKET_ID_REQUEST_CONTROLLER_COUNT)?;
    let mut cursor = Cursor::new(&packet.payload);
    cursor.read_u32()
}

fn request_controller_data(
    stream: &mut TcpStream,
    idx: u32,
    protocol_version: u32,
) -> Result<DeviceData, String>
{
    if protocol_version >= 1 {
        let payload = protocol_version.to_le_bytes();
        send_packet(
            stream,
            idx,
            PACKET_ID_REQUEST_CONTROLLER_DATA,
            &payload,
        )?;
    } else {
        send_packet(stream, idx, PACKET_ID_REQUEST_CONTROLLER_DATA, &[])?;
    }

    let packet = read_packet_expect(stream, PACKET_ID_REQUEST_CONTROLLER_DATA)?;

    parse_controller_data(idx, &packet.payload, protocol_version)
}

fn select_keyboard(devices: Vec<DeviceData>) -> Result<DeviceData, String>
{
    let mut keyboards: Vec<DeviceData> = devices
        .into_iter()
        .filter(|device| device.device_type == DEVICE_TYPE_KEYBOARD)
        .collect();

    if keyboards.is_empty() {
        return Err("OpenRGB did not report any keyboard devices.".to_string());
    }

    if let Some(index) = keyboards.iter().position(|device| {
        device.vendor.to_ascii_lowercase().contains("corsair")
            || device.display_name.to_ascii_lowercase().contains("corsair")
    }) {
        return Ok(keyboards.swap_remove(index));
    }

    Ok(keyboards.remove(0))
}

fn build_led_map(led_names: &[String], led_alt_names: &[String]) -> HashMap<char, u32>
{
    let mut map = HashMap::new();

    for (idx, name) in led_alt_names.iter().enumerate() {
        if idx >= led_names.len() {
            break;
        }
        if let Some(ch) = extract_char(name) {
            map.entry(ch).or_insert(idx as u32);
        }
    }

    for (idx, name) in led_names.iter().enumerate() {
        if let Some(ch) = extract_char(name) {
            map.entry(ch).or_insert(idx as u32);
        }
    }

    map
}

fn extract_char(name: &str) -> Option<char>
{
    let mut value = name.trim().to_ascii_uppercase();
    if value.contains("SPACE") {
        return Some(' ');
    }
    if let Some(stripped) = value.strip_prefix("KEY:") {
        value = stripped.trim().to_string();
    } else if let Some(stripped) = value.strip_prefix("KEY") {
        value = stripped.trim().to_string();
    }

    for token in value.split(|c: char| !c.is_ascii_alphanumeric()) {
        if token.len() == 1 {
            let ch = token.chars().next()?;
            if ch.is_ascii_alphanumeric() {
                return Some(ch);
            }
        }
    }

    None
}

fn send_update_leds(
    stream: &mut TcpStream,
    device_idx: u32,
    colors: &[u32],
) -> Result<(), String>
{
    let color_count = colors.len().min(u16::MAX as usize) as u16;
    let mut payload = Vec::with_capacity(6 + colors.len() * 4);
    let data_size = 4u32 + 2u32 + (color_count as u32) * 4;
    payload.extend_from_slice(&data_size.to_le_bytes());
    payload.extend_from_slice(&color_count.to_le_bytes());
    for &color in colors.iter().take(color_count as usize) {
        payload.extend_from_slice(&color.to_le_bytes());
    }

    send_packet(stream, device_idx, PACKET_ID_UPDATE_LEDS, &payload)
}

fn send_packet(
    stream: &mut TcpStream,
    device_idx: u32,
    packet_id: u32,
    payload: &[u8],
) -> Result<(), String>
{
    let mut header = Vec::with_capacity(16 + payload.len());
    header.extend_from_slice(PACKET_MAGIC);
    header.extend_from_slice(&device_idx.to_le_bytes());
    header.extend_from_slice(&packet_id.to_le_bytes());
    header.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    header.extend_from_slice(payload);
    stream
        .write_all(&header)
        .map_err(|err| format!("Failed to send OpenRGB packet {packet_id}: {err}"))?;
    stream
        .flush()
        .map_err(|err| format!("Failed to flush OpenRGB packet {packet_id}: {err}"))?;
    Ok(())
}

struct Packet
{
    packet_id: u32,
    payload: Vec<u8>,
}

fn read_packet(stream: &mut TcpStream) -> Result<Packet, String>
{
    let mut header = [0u8; 16];
    stream
        .read_exact(&mut header)
        .map_err(|err| format!("Failed to read OpenRGB packet header: {err}"))?;

    if &header[..4] != PACKET_MAGIC {
        return Err("OpenRGB packet magic mismatch".to_string());
    }

    let _device_idx = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    let packet_id = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
    let payload_size = u32::from_le_bytes([header[12], header[13], header[14], header[15]]) as usize;

    let mut payload = vec![0u8; payload_size];
    if payload_size > 0 {
        stream
            .read_exact(&mut payload)
            .map_err(|err| format!("Failed to read OpenRGB packet payload: {err}"))?;
    }

    Ok(Packet { packet_id, payload })
}

fn try_read_packet(stream: &mut TcpStream) -> Result<Option<Packet>, String>
{
    let mut header = [0u8; 16];
    match stream.read_exact(&mut header) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::TimedOut => return Ok(None),
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
        Err(err) => return Err(format!("Failed to read OpenRGB packet header: {err}")),
    }

    if &header[..4] != PACKET_MAGIC {
        return Err("OpenRGB packet magic mismatch".to_string());
    }

    let _device_idx = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    let packet_id = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
    let payload_size = u32::from_le_bytes([header[12], header[13], header[14], header[15]]) as usize;

    let mut payload = vec![0u8; payload_size];
    if payload_size > 0 {
        stream
            .read_exact(&mut payload)
            .map_err(|err| format!("Failed to read OpenRGB packet payload: {err}"))?;
    }

    Ok(Some(Packet { packet_id, payload }))
}

fn read_packet_expect(stream: &mut TcpStream, expected_id: u32) -> Result<Packet, String>
{
    for _ in 0..5 {
        let packet = read_packet(stream)?;
        if packet.packet_id == expected_id {
            return Ok(packet);
        }
    }

    Err(format!(
        "Did not receive expected OpenRGB packet id {expected_id}"
    ))
}

fn parse_controller_data(
    idx: u32,
    payload: &[u8],
    protocol_version: u32,
) -> Result<DeviceData, String>
{
    let mut cursor = Cursor::new(payload);
    let _data_size = cursor.read_u32()?;
    let device_type = cursor.read_i32()?;
    let name = cursor.read_string()?;
    let vendor = if protocol_version >= 1 {
        cursor.read_string()?
    } else {
        String::new()
    };
    let _description = cursor.read_string()?;
    let _version = cursor.read_string()?;
    let _serial = cursor.read_string()?;
    let _location = cursor.read_string()?;
    let num_modes = cursor.read_u16()?;
    let _active_mode = cursor.read_i32()?;
    for _ in 0..num_modes {
        skip_mode_data(&mut cursor, protocol_version)?;
    }

    let num_zones = cursor.read_u16()?;
    for _ in 0..num_zones {
        skip_zone_data(&mut cursor, protocol_version)?;
    }

    let num_leds = cursor.read_u16()?;
    let mut led_names = Vec::with_capacity(num_leds as usize);
    for _ in 0..num_leds {
        let led_name = cursor.read_string()?;
        let _led_value = cursor.read_u32()?;
        led_names.push(led_name);
    }

    let num_colors = cursor.read_u16()?;
    cursor.skip((num_colors as usize) * 4)?;

    let mut led_alt_names = Vec::new();
    if protocol_version >= 5 {
        let alt_count = cursor.read_u16()?;
        for _ in 0..alt_count {
            led_alt_names.push(cursor.read_string()?);
        }
        let _flags = cursor.read_u32()?;
    }

    let display_name = if vendor.is_empty() {
        name.clone()
    } else {
        format!("{} {}", vendor, name)
    };

    Ok(DeviceData {
        idx,
        device_type,
        display_name,
        vendor,
        led_names,
        led_alt_names,
    })
}

fn skip_mode_data(cursor: &mut Cursor, protocol_version: u32) -> Result<(), String>
{
    let _name = cursor.read_string()?;
    let _mode_value = cursor.read_i32()?;
    let _mode_flags = cursor.read_u32()?;
    let _mode_speed_min = cursor.read_u32()?;
    let _mode_speed_max = cursor.read_u32()?;
    if protocol_version >= 3 {
        let _brightness_min = cursor.read_u32()?;
        let _brightness_max = cursor.read_u32()?;
    }
    let _mode_colors_min = cursor.read_u32()?;
    let _mode_colors_max = cursor.read_u32()?;
    let _mode_speed = cursor.read_u32()?;
    if protocol_version >= 3 {
        let _brightness = cursor.read_u32()?;
    }
    let _mode_direction = cursor.read_u32()?;
    let _mode_color_mode = cursor.read_u32()?;
    let num_colors = cursor.read_u16()?;
    cursor.skip((num_colors as usize) * 4)?;
    Ok(())
}

fn skip_zone_data(cursor: &mut Cursor, protocol_version: u32) -> Result<(), String>
{
    let _name = cursor.read_string()?;
    let _zone_type = cursor.read_i32()?;
    let _zone_leds_min = cursor.read_u32()?;
    let _zone_leds_max = cursor.read_u32()?;
    let _zone_leds_count = cursor.read_u32()?;
    let matrix_len = cursor.read_u16()? as usize;
    if matrix_len > 0 {
        let _height = cursor.read_u32()?;
        let _width = cursor.read_u32()?;
        let remaining = matrix_len.saturating_sub(8);
        cursor.skip(remaining)?;
    }

    if protocol_version >= 4 {
        let num_segments = cursor.read_u16()?;
        for _ in 0..num_segments {
            skip_segment_data(cursor)?;
        }
    }

    if protocol_version >= 5 {
        let _zone_flags = cursor.read_u32()?;
    }

    Ok(())
}

fn skip_segment_data(cursor: &mut Cursor) -> Result<(), String>
{
    let _name = cursor.read_string()?;
    let _segment_type = cursor.read_i32()?;
    let _start_idx = cursor.read_u32()?;
    let _leds_count = cursor.read_u32()?;
    Ok(())
}

fn rgb_to_u32(color: RgbColor) -> u32
{
    ((color.b as u32) << 16) | ((color.g as u32) << 8) | (color.r as u32)
}

struct Cursor<'a>
{
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a>
{
    fn new(buf: &'a [u8]) -> Self
    {
        Self { buf, pos: 0 }
    }

    fn read_u16(&mut self) -> Result<u16, String>
    {
        let bytes = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn read_u32(&mut self) -> Result<u32, String>
    {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_i32(&mut self) -> Result<i32, String>
    {
        let bytes = self.read_bytes(4)?;
        Ok(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_string(&mut self) -> Result<String, String>
    {
        let len = self.read_u16()? as usize;
        let bytes = self.read_bytes(len)?;
        let trimmed = bytes
            .split(|&b| b == 0)
            .next()
            .unwrap_or(&[]);
        Ok(String::from_utf8_lossy(trimmed).to_string())
    }

    fn skip(&mut self, len: usize) -> Result<(), String>
    {
        self.read_bytes(len)?;
        Ok(())
    }

    fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], String>
    {
        let end = self.pos + len;
        if end > self.buf.len() {
            return Err("OpenRGB packet parse error: unexpected end of buffer".to_string());
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }
}
