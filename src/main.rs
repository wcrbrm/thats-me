use std::fmt;
use std::io;
use std::sync::{mpsc, Arc, Mutex, RwLock};
use std::thread;
// use std::time::Instant;

use jpeg_decoder as jpeg;
use softbuffer::GraphicsContext;
use winit::event::{Event, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::{WindowBuilder, WindowLevel};

use v4l::buffer::Type;
use v4l::io::traits::CaptureStream;
use v4l::prelude::*;
use v4l::video::capture::Parameters;
use v4l::video::Capture;
use v4l::{Format, FourCC};

#[derive(Default)]
struct State {
    total_received: u32,
    total_rendered: u32,
    prev_rendered: u32,
    prev_received: u32,
    window_title: String,
}

impl fmt::Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let received = self.total_received - self.prev_received;
        write!(f, "My Camera ({} fps)", received)
    }
}

fn main() -> io::Result<()> {
    let path = "/dev/video0";
    println!("Using device: {}\n", path);

    let buffer_count = 4;
    let mut format: Format;
    let params: Parameters;
    let dev = RwLock::new(Device::with_path(path)?);
    {
        let dev = dev.write().unwrap();
        format = dev.format()?;
        params = dev.params()?;
        // try RGB3 first
        format.fourcc = FourCC::new(b"RGB3");
        format = dev.set_format(&format)?;
        if format.fourcc != FourCC::new(b"RGB3") {
            // fallback to Motion-JPEG
            format.fourcc = FourCC::new(b"MJPG");
            format = dev.set_format(&format)?;

            if format.fourcc != FourCC::new(b"MJPG") {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "neither RGB3 nor MJPG supported by the device, but required by this example!",
                ));
            }
        }
    }

    println!("Active format:\n{}", format);
    println!("Active parameters:\n{}", params);
    let scale = 1 as usize;
    let event_loop = EventLoop::new();
    let sz = format.height / scale as u32;
    let pad = 20u32;
    let top_bar_height = 40;
    let screen_size = (1920, 1080);
    let title = "My Camera";
    let inner_size = winit::dpi::PhysicalSize::new(sz, sz);

    let window = WindowBuilder::new()
        .with_title(title)
        .with_inner_size(inner_size)
        .with_resizable(false)
        .with_transparent(false)
        .with_position(winit::dpi::PhysicalPosition::new(
            screen_size.0 - sz - pad,
            screen_size.1 - sz - pad - top_bar_height,
        ))
        .build(&event_loop)
        .unwrap();
    let wid = window.id();
    window.set_window_level(WindowLevel::AlwaysOnTop);

    let (tx, rx) = mpsc::channel();
    let mut graphics_context = unsafe { GraphicsContext::new(&window, &window) }.unwrap();

    let state = Arc::new(Mutex::new(State::default()));
    let state_fps_counter = state.clone();
    thread::spawn(move || loop {
        thread::sleep(std::time::Duration::from_secs(1));
        let mut state = state_fps_counter.lock().unwrap();
        println!("{:?}", state);
        let received = state.total_received - state.prev_received;
        state.window_title = format!("My Camera ({} fps)", received);
        state.prev_received = state.total_received;
        state.prev_rendered = state.total_rendered;
    });

    let state_camera_stream = state.clone();
    thread::spawn(move || {
        let dev = dev.write().unwrap();
        // Setup a buffer stream
        let mut stream = MmapStream::with_buffers(&dev, Type::VideoCapture, buffer_count).unwrap();
        loop {
            let (buf, _) = stream.next().unwrap();
            let camera_data = match &format.fourcc.repr {
                b"RGB3" => buf.to_vec(),
                b"MJPG" => {
                    // Decode the JPEG frame to RGB
                    let mut decoder = jpeg::Decoder::new(buf);
                    decoder.decode().expect("failed to decode JPEG")
                }
                _ => panic!("invalid buffer pixelformat"),
            };

            let sw = format.width as usize * 3;
            let xoffs = sw / 4;
            let mut y = 0 as usize;
            let mut x = 0 as usize;
            let (width, height) = (inner_size.width, inner_size.height);
            let total = (width * height) as usize;
            let screen_data = (0..(total))
                .map(|_index| {
                    let orig_x = x * scale * 3 + xoffs;
                    let orig_y = y * scale;
                    let sindex = (orig_x + orig_y * sw) as usize;
                    let red = camera_data[sindex] as u32;
                    let green = camera_data[sindex + 1] as u32;
                    let blue = camera_data[sindex + 2] as u32;
                    let color = blue | (green << 8) | (red << 16);

                    x += 1;
                    if x >= width as usize {
                        x = 0;
                        y += 1;
                    }
                    color as u32
                })
                .collect::<Vec<_>>();
            tx.send(screen_data).unwrap();

            let mut state = state_camera_stream.lock().unwrap();
            state.total_received += 1;
            window.set_title(&state.window_title);
            window.request_redraw();
        }
    });

    let state_event_loop = state.clone();
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::RedrawRequested(window_id) if window_id == wid => {
                let buffer = rx.recv().unwrap();
                let (width, height) = (inner_size.width, inner_size.height);
                graphics_context.set_buffer(&buffer, width as u16, height as u16);

                let mut state = state_event_loop.lock().unwrap();
                state.total_rendered += 1;
                // println!("redrawn: {}x{} ", width, height);
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                window_id,
            } if window_id == wid => {
                *control_flow = ControlFlow::Exit;
            }
            _ => {} // window.request_redraw(),
        }
    });
}
