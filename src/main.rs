use std::fmt::{Display, Formatter};
use std::io::Write;
use std::path::Path;

use actix_web::web::Bytes;
use actix_web::{get, post, web, App, HttpResponse, HttpServer, ResponseError};
use anyhow::{anyhow, bail};
use futures::task::Context;
use futures::{Stream, StreamExt};
use log::{debug, error, info, warn};
use systemd_socket_activation::systemd_socket_activation;
use tokio::macros::support::{Pin, Poll};
use tokio::sync::mpsc::UnboundedReceiver;

use sane_scan as sane;

mod frontend_files;

macro_rules! try_or_send {
	($v:expr, $sender:ident) => {
		match $v {
			Ok(v) => v,
			Err(e) => {
				$sender.send(Err(e.into())).unwrap();
				return;
			}
		}
	};
}

const fn log_str() -> &'static str {
	if cfg!(debug_assertions) {
		"debug"
	} else {
		"info"
	}
}

fn main() -> anyhow::Result<()> {
	flexi_logger::Logger::try_with_env_or_str(log_str())?.start()?;

	if std::env::args().skip(1).any(|arg| arg == "--just-scan") {
		return scan_to_file();
	}

	run_webserver()
}

fn scan_to_file() -> anyhow::Result<()> {
	let mut image = scan()?;

	info!("Scan completed. Saving to file.");

	rgb_to_bgr(&mut image.raw_data);

	save_as_bmp(
		"./scanned_document.bmp".as_ref(),
		&image.raw_data,
		(image.width, image.height),
	)?;

	Ok(())
}

#[actix_web::main]
async fn run_webserver() -> anyhow::Result<()> {
	debug!("Frontend files:");
	for (filename, _content) in frontend_files::FRONTEND_FILES.iter() {
		debug!("\t{}", filename);
	}

	let mut http_server = HttpServer::new(|| {
		App::new()
			.service(scan_service)
			.service(echo_service)
			.service(frontend_files::Service)
	});

	let port = std::env::args()
		.skip(1)
		.find(|arg| arg.starts_with("-p="))
		.and_then(|port| port.strip_prefix("-p=").map(ToString::to_string))
		.unwrap_or_else(|| "8000".to_string())
		.parse::<u16>()?;

	let (address, port) = ("0.0.0.0", port);

	match systemd_socket_activation() {
		Ok(sockets) if !sockets.is_empty() => {
			info!("Using systemd provided sockets instead");
			for socket in sockets {
				http_server = http_server.listen(socket)?;
			}
		}
		Err(systemd_socket_activation::Error::LibLoadingFailedToLoadSystemd(e))
			if cfg!(target_os = "linux") =>
		{
			warn!("libsystemd not found: {}", e);
			http_server = http_server.bind(format!("{}:{}", address, port))?;
		}
		Err(e) => {
			error!("Systemd socket activation failed: {:?}", e);
			http_server = http_server.bind(format!("{}:{}", address, port))?;
		}
		// Call to systemd was successful, but didn't return any sockets
		Ok(_) => {
			http_server = http_server.bind(format!("{}:{}", address, port))?;
		}
	}

	http_server.run().await?;

	Ok(())
}

#[derive(Debug)]
struct ScanServiceError(anyhow::Error);

impl Display for ScanServiceError {
	fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
		write!(f, "{:?}", self)
	}
}

impl ResponseError for ScanServiceError {}

impl From<anyhow::Error> for ScanServiceError {
	fn from(e: anyhow::Error) -> Self {
		ScanServiceError(e)
	}
}

#[post("/echo")]
async fn echo_service(mut payload: web::Payload) -> Result<Bytes, actix_web::Error> {
	let mut body = web::BytesMut::new();
	while let Some(chunk) = payload.next().await {
		let chunk = chunk?;
		body.extend_from_slice(&chunk);
	}

	debug!("Received {:?}. Echoing it", &body);

	Ok(body.freeze())
}

#[get("/scan.bmp")]
async fn scan_service() -> Result<HttpResponse, ScanServiceError> {
	// TODO(aqatl): Possibly via a websocket?
	let scan_stream = scan_stream_bmp().await;

	Ok(HttpResponse::Ok()
		.content_type("image/bmp")
		.streaming(scan_stream))
}

struct ScanImage {
	raw_data: Vec<u8>,
	width: u32,
	height: u32,
}

fn scan() -> anyhow::Result<ScanImage> {
	let sane = sane::Sane::init_1_0()?;
	let devices = sane.get_devices()?;
	info!("devices: {:#?}", devices);

	if devices.is_empty() {
		bail!("No scanners found");
	}

	let mut handle = devices[0].open()?;

	let device_options = handle.get_options()?;
	info!("Device options: {:#?}", device_options);

	if let Some(res_opt) = device_options
		.iter()
		.find(|opt| opt.name.to_bytes() == b"resolution")
	{
		if let sane::OptionConstraint::WordList(ref resolutions) = res_opt.constraint {
			info!(
				"Available resolutions: {:?}. Unit: {:?}",
				resolutions, res_opt.unit
			);
			if matches!(res_opt.unit, sane::sys::Unit::Dpi) && resolutions.contains(&300) {
				info!("Setting resolution to 300 DPI");
				let info = handle.set_option(res_opt, sane::DeviceOptionValue::Int(300))?;
				info!("Returned info: {:#?}.", info);

				let new_res = handle.get_option(res_opt)?;
				info!("Resolution set to {:?} {:?}", new_res, res_opt.unit);
			}
		}
	}

	let parameters = handle.start_scan()?;

	let width = parameters.pixels_per_line as u32;
	let height = parameters.lines as u32;

	println!("Width: {} Height: {}", width, height);

	let image = handle.read_to_vec()?;

	Ok(ScanImage {
		raw_data: image,
		width,
		height,
	})
}

async fn scan_stream_bmp() -> StreamingReceiver<Result<Bytes, anyhow::Error>> {
	let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<Result<Bytes, anyhow::Error>>();

	tokio::task::spawn_blocking(move || {
		let libsane: sane::Sane = try_or_send!(sane::Sane::init_1_0(), sender);
		let devices: Vec<sane::Device> = try_or_send!(libsane.get_devices(), sender);

		if devices.is_empty() {
			sender.send(Err(anyhow!("No scanners found."))).unwrap();
			return;
		}

		let mut handle = try_or_send!(devices[0].open(), sender);

		let device_options = try_or_send!(handle.get_options(), sender);
		info!("Device options: {:#?}", device_options);

		if let Some(res_opt) = device_options
			.iter()
			.find(|opt| opt.name.to_bytes() == b"resolution")
		{
			if let sane::OptionConstraint::WordList(ref resolutions) = res_opt.constraint {
				info!(
					"Available resolutions: {:?}. Unit: {:?}",
					resolutions, res_opt.unit
				);
				if matches!(res_opt.unit, sane::sys::Unit::Dpi) && resolutions.contains(&300) {
					info!("Setting resolution to 300 DPI");
					let info = try_or_send!(
						handle.set_option(res_opt, sane::DeviceOptionValue::Int(300)),
						sender
					);
					info!("Returned info: {:#?}.", info);

					let new_res = try_or_send!(handle.get_option(res_opt), sender);
					info!("Resolution set to {:?} {:?}", new_res, res_opt.unit);
				}
			}
		}

		let parameters = try_or_send!(handle.start_scan(), sender);

		let width = parameters.pixels_per_line as u32;
		let height = parameters.lines as u32;

		let mut bmp_header = Vec::with_capacity(BMP_HEADER_SIZE as usize);
		let encode_as_bmp = |img_len: u32, (width, height): (u32, u32), out: &mut Vec<_>| {
			out.write_all(&[b'B', b'M'])?;
			let file_size = BMP_HEADER_SIZE + img_len;
			out.write_all(&file_size.to_le_bytes())?;
			out.write_all(&[0, 0])?;
			out.write_all(&[0, 0])?;
			out.write_all(&BMP_HEADER_SIZE.to_le_bytes())?;

			// image header
			out.write_all(&BMP_IMAGE_HEADER_SIZE.to_le_bytes())?;
			out.write_all(&width.to_le_bytes())?;
			out.write_all(&(-(height as i32)).to_le_bytes())?;
			out.write_all(&1_u16.to_le_bytes())?;
			out.write_all(&24_u16.to_le_bytes())?;
			out.write_all(&0_u32.to_le_bytes())?;
			out.write_all(&0_u32.to_le_bytes())?;
			out.write_all(&0_u32.to_le_bytes())?;
			out.write_all(&0_u32.to_le_bytes())?;
			out.write_all(&0_u32.to_le_bytes())?;
			out.write_all(&0_u32.to_le_bytes())?;

			Result::<(), std::io::Error>::Ok(())
		};
		try_or_send!(
			encode_as_bmp(width * height * 3, (width, height), &mut bmp_header),
			sender
		);
		sender.send(Ok(Bytes::from(bmp_header))).unwrap();

		// Vector capacity must be divisible by 3, so that we always get full pixels. Otherwise, we'd
		// get color artifacts when switching from RGB to BGR
		let mut buf = Vec::<u8>::with_capacity(3 * 1024 * 1024);

		unsafe {
			buf.set_len(buf.capacity());
		}

		while let Ok(Some(written)) = handle.read(buf.as_mut_slice()) {
			let mut cloned_buf = (&buf[0..written]).to_vec();
			rgb_to_bgr(&mut cloned_buf);
			sender.send(Ok(Bytes::from(cloned_buf))).unwrap();
		}
	});
	StreamingReceiver(receiver)
}

struct StreamingReceiver<T>(UnboundedReceiver<T>);

impl<T> Stream for StreamingReceiver<T> {
	type Item = T;

	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		self.0.poll_recv(cx)
	}
}

const BMP_FILE_HEADER_SIZE: u32 = 2 + 4 + 2 + 2 + 4;
const BMP_IMAGE_HEADER_SIZE: u32 = 4 + 4 + 4 + 2 + 2 + 4 + 4 + 4 + 4 + 4 + 4;
const BMP_HEADER_SIZE: u32 = BMP_FILE_HEADER_SIZE + BMP_IMAGE_HEADER_SIZE;

fn encode_as_bmp<W: Write>(
	img: &[u8],
	(width, height): (u32, u32),
	out: &mut W,
) -> std::io::Result<()> {
	// file header
	out.write_all(&[b'B', b'M'])?;
	let file_size = BMP_HEADER_SIZE + img.len() as u32;
	out.write_all(&file_size.to_le_bytes())?;
	out.write_all(&[0, 0])?;
	out.write_all(&[0, 0])?;
	out.write_all(&BMP_HEADER_SIZE.to_le_bytes())?;

	// image header
	out.write_all(&BMP_IMAGE_HEADER_SIZE.to_le_bytes())?;
	out.write_all(&width.to_le_bytes())?;
	out.write_all(&(-(height as i32)).to_le_bytes())?;
	out.write_all(&1_u16.to_le_bytes())?;
	out.write_all(&24_u16.to_le_bytes())?;
	out.write_all(&0_u32.to_le_bytes())?;
	out.write_all(&0_u32.to_le_bytes())?;
	out.write_all(&0_u32.to_le_bytes())?;
	out.write_all(&0_u32.to_le_bytes())?;
	out.write_all(&0_u32.to_le_bytes())?;
	out.write_all(&0_u32.to_le_bytes())?;

	out.write_all(img)?;

	Ok(())
}

fn save_as_bmp(path: &Path, img: &[u8], (width, height): (u32, u32)) -> std::io::Result<()> {
	let mut file = std::fs::File::create(path)?;
	encode_as_bmp(img, (width, height), &mut file)?;
	Ok(())
}

fn rgb_to_bgr(image: &mut [u8]) {
	for chunk in image.chunks_exact_mut(3) {
		chunk.swap(0, 2);
	}
}

#[allow(dead_code)]
fn display_parameters(sane_parameters: &sane::Parameters) {
	info!("Print parameters:");
	info!("\tFormat {:?}.", sane_parameters.format);
	info!("\tLast Frame {}.", sane_parameters.last_frame);
	info!("\tBytes per line {}.", sane_parameters.bytes_per_line);
	info!("\tPixels per line {}.", sane_parameters.pixels_per_line);
	info!("\tLines {}.", sane_parameters.lines);
	info!("\tDepth {}.", sane_parameters.depth);
}

#[allow(dead_code)]
fn display_options(options: &[sane::DeviceOption]) {
	for option in options {
		info!("{:?}", option.title.to_string_lossy());
		info!("\t{:?}", option.name.to_string_lossy());
		info!("\t{:?}", option.desc.to_string_lossy());

		info!(
			"Type {:?}. Unit {:?}. Size {:?}. Cap {:?}. Constraint type {:?}",
			option.type_, option.unit, option.size, option.cap, option.constraint
		);
		match &option.constraint {
			sane::OptionConstraint::None => (),
			sane::OptionConstraint::Range { range, quant } => {
				info!(
					"\tRange: {}-{}. Quant {}. Steps {}.",
					range.start,
					range.end,
					quant,
					range.end / quant,
				);
			}
			sane::OptionConstraint::WordList(list) => {
				info!("\tPossible values: {:?}", list);
			}
			sane::OptionConstraint::StringList(list) => {
				info!("\tPossible values: {:?}", list);
			}
		}
	}
}
