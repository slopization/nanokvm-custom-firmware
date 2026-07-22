use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::Path;

use clap::ValueEnum;
use drm::buffer::{Buffer, DrmFourcc};
use drm::control::dumbbuffer::DumbBuffer;
use drm::control::{ClipRect, Device as ControlDevice, connector, framebuffer};
use tokio::sync::watch;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayStatus {
    pub otp: String,
    pub ip_address: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DisplayTransform {
    Identity,
    MirrorX,
}

struct Card(File);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl drm::Device for Card {}
impl ControlDevice for Card {}

pub async fn run_display(
    device: &Path,
    transform: DisplayTransform,
    mut status: watch::Receiver<Option<DisplayStatus>>,
) -> io::Result<()> {
    let path = device.to_owned();
    let mut display = tokio::task::spawn_blocking(move || OledDisplay::open(&path, transform))
        .await
        .map_err(io::Error::other)??;
    loop {
        let current = status.borrow_and_update().clone();
        display.render(current.as_ref())?;
        if status.changed().await.is_err() {
            return Ok(());
        }
    }
}

struct OledDisplay {
    card: Card,
    buffer: DumbBuffer,
    framebuffer: framebuffer::Handle,
    width: usize,
    height: usize,
    pitch: usize,
    transform: DisplayTransform,
}

impl OledDisplay {
    fn open(path: &Path, transform: DisplayTransform) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let card = Card(file);
        let resources = card.resource_handles()?;
        let connector = connected_connector(&card, resources.connectors())?;
        let mode = *connector
            .modes()
            .first()
            .ok_or_else(|| io::Error::other("OLED connector has no mode"))?;
        let crtc = resources
            .crtcs()
            .first()
            .copied()
            .ok_or_else(|| io::Error::other("OLED has no CRTC"))?;
        let (width, height) = mode.size();
        let mut buffer = card.create_dumb_buffer(
            (u32::from(width), u32::from(height)),
            DrmFourcc::Xrgb8888,
            32,
        )?;
        let pitch = buffer.pitch() as usize;
        card.map_dumb_buffer(&mut buffer)?.as_mut().fill(0);
        let framebuffer = card.add_framebuffer(&buffer, 24, 32)?;
        card.set_crtc(
            crtc,
            Some(framebuffer),
            (0, 0),
            &[connector.handle()],
            Some(mode),
        )?;
        Ok(Self {
            card,
            buffer,
            framebuffer,
            width: width.into(),
            height: height.into(),
            pitch,
            transform,
        })
    }

    fn render(&mut self, status: Option<&DisplayStatus>) -> io::Result<()> {
        let width = self.width;
        let height = self.height;
        let pitch = self.pitch;
        let mut mapping = self.card.map_dumb_buffer(&mut self.buffer)?;
        mapping.as_mut().fill(0);
        if let Some(status) = status {
            draw_centered(
                mapping.as_mut(),
                pitch,
                width,
                height,
                self.transform,
                1,
                "RECOVERY",
            );
            draw_centered(
                mapping.as_mut(),
                pitch,
                width,
                height,
                self.transform,
                11,
                &status.otp,
            );
            draw_centered(
                mapping.as_mut(),
                pitch,
                width,
                height,
                self.transform,
                22,
                &status.ip_address,
            );
        }
        drop(mapping);
        self.card.dirty_framebuffer(
            self.framebuffer,
            &[ClipRect::new(0, 0, self.width as u16, self.height as u16)],
        )?;
        Ok(())
    }
}

impl Drop for OledDisplay {
    fn drop(&mut self) {
        let _ = self.card.destroy_framebuffer(self.framebuffer);
        let _ = self.card.destroy_dumb_buffer(self.buffer);
    }
}

fn connected_connector(card: &Card, handles: &[connector::Handle]) -> io::Result<connector::Info> {
    handles
        .iter()
        .filter_map(|handle| card.get_connector(*handle, true).ok())
        .find(|connector| connector.state() == connector::State::Connected)
        .ok_or_else(|| io::Error::other("OLED connector is not connected"))
}

fn draw_centered(
    pixels: &mut [u8],
    pitch: usize,
    width: usize,
    height: usize,
    transform: DisplayTransform,
    y: usize,
    text: &str,
) {
    let text_width = text.chars().count().saturating_mul(5).saturating_sub(1);
    let x = width.saturating_sub(text_width) / 2;
    for (index, character) in text.chars().enumerate() {
        draw_glyph(
            pixels,
            pitch,
            width,
            height,
            transform,
            x + index * 5,
            y,
            character,
        );
    }
}

fn draw_glyph(
    pixels: &mut [u8],
    pitch: usize,
    width: usize,
    height: usize,
    transform: DisplayTransform,
    x: usize,
    y: usize,
    character: char,
) {
    for (row, bits) in glyph(character).iter().copied().enumerate() {
        for column in 0..4 {
            if bits & (1 << (3 - column)) != 0 {
                set_pixel(pixels, pitch, width, height, transform, x + column, y + row);
            }
        }
    }
}

fn set_pixel(
    pixels: &mut [u8],
    pitch: usize,
    width: usize,
    height: usize,
    transform: DisplayTransform,
    x: usize,
    y: usize,
) {
    if x >= width || y >= height {
        return;
    }
    let x = match transform {
        DisplayTransform::Identity => x,
        DisplayTransform::MirrorX => width - 1 - x,
    };
    let offset = y * pitch + x * 4;
    pixels[offset..offset + 4].copy_from_slice(&[0xff, 0xff, 0xff, 0]);
}

fn glyph(character: char) -> [u8; 7] {
    match character.to_ascii_uppercase() {
        '0' => [0x6, 0x9, 0xb, 0xd, 0x9, 0x9, 0x6],
        '1' => [0x2, 0x6, 0x2, 0x2, 0x2, 0x2, 0x7],
        '2' => [0x6, 0x9, 0x1, 0x2, 0x4, 0x8, 0xf],
        '3' => [0xe, 0x1, 0x1, 0x6, 0x1, 0x1, 0xe],
        '4' => [0x2, 0x6, 0xa, 0xa, 0xf, 0x2, 0x2],
        '5' => [0xf, 0x8, 0x8, 0xe, 0x1, 0x1, 0xe],
        '6' => [0x6, 0x8, 0x8, 0xe, 0x9, 0x9, 0x6],
        '7' => [0xf, 0x1, 0x2, 0x2, 0x4, 0x4, 0x4],
        '8' => [0x6, 0x9, 0x9, 0x6, 0x9, 0x9, 0x6],
        '9' => [0x6, 0x9, 0x9, 0x7, 0x1, 0x1, 0x6],
        'A' => [0x6, 0x9, 0x9, 0xf, 0x9, 0x9, 0x9],
        'C' => [0x7, 0x8, 0x8, 0x8, 0x8, 0x8, 0x7],
        'E' => [0xf, 0x8, 0x8, 0xe, 0x8, 0x8, 0xf],
        'O' => [0x6, 0x9, 0x9, 0x9, 0x9, 0x9, 0x6],
        'R' => [0xe, 0x9, 0x9, 0xe, 0xa, 0x9, 0x9],
        'V' => [0x9, 0x9, 0x9, 0x9, 0x9, 0x6, 0x6],
        'Y' => [0x9, 0x9, 0x6, 0x2, 0x2, 0x2, 0x2],
        '.' => [0, 0, 0, 0, 0, 0x6, 0x6],
        '-' => [0, 0, 0, 0xf, 0, 0, 0],
        _ => [0; 7],
    }
}
