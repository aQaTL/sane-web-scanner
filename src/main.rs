use std::ffi::{c_void, CStr};
use std::io::Write;
use std::path::Path;

use actix_web::web::Bytes;
use actix_web::{get, App, HttpResponse, HttpServer, ResponseError};
use anyhow::{anyhow, bail};
use futures::task::Context;
use futures::Stream;
use log::{debug, error, info, warn};
use std::fmt::{Display, Formatter};
use systemd_socket_activation::systemd_socket_activation;
use tokio::macros::support::{Pin, Poll};
use tokio::sync::mpsc::UnboundedReceiver;

mod sane {
	//! API docs: <https://sane-project.gitlab.io/standard/1.06/api.html>

	#![allow(non_upper_case_globals)]
	#![allow(non_camel_case_types)]
	#![allow(non_snake_case)]
	#![allow(dead_code)]
	#![allow(clippy::unused_unit)]

	include!(concat!(env!("OUT_DIR"), "/sane.rs"));

	impl Drop for libsane {
		fn drop(&mut self) {
			unsafe {
				log::debug!("Drop libsane.");
				self.sane_exit();
			}
		}
	}
}

struct Device<'a>(pub sane::SANE_Handle, &'a sane::libsane);

unsafe impl Send for Device<'_> {}
unsafe impl Sync for Device<'_> {}

impl<'a> Device<'a> {
	pub unsafe fn from_raw_handle(handle: sane::SANE_Handle, libsane: &'a sane::libsane) -> Self {
		Device(handle, libsane)
	}
}

impl Drop for Device<'_> {
	fn drop(&mut self) {
		debug!("Drop Device.");
		unsafe { self.1.sane_close(self.0) }
	}
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
	let mut http_server = HttpServer::new(|| App::new().service(scan_service));

	let port = std::env::args()
		.skip(1)
		.find(|arg| arg.starts_with("-p="))
		.and_then(|port| port.strip_prefix("-p=").map(ToString::to_string))
		.unwrap_or("8000".to_string())
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

#[get("/scan.bmp")]
async fn scan_service() -> Result<HttpResponse, ScanServiceError> {
	// TODO(aqatl): Possibly via a websocket?
	let scan_stream = scan_stream_bmp().await;

	// info!("Scan completed. Saving to file.");

	// // BMP is BGR by default, while our image is assumed to be RGB. This swaps red channel with blue.
	// rgb_to_bgr(&mut image.raw_data);

	// save_as_bmp(
	// 	"./scanned_document.bmp".as_ref(),
	// 	&image.raw_data,
	// 	(image.width, image.height),
	// )
	// .unwrap();

	// let mut bmp_img = Vec::with_capacity(image.raw_data.len() + BMP_HEADER_SIZE as usize);
	// encode_as_bmp(&image.raw_data, (image.width, image.height), &mut bmp_img).unwrap();

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
	let libsane = init_libsane()?;
	let (device_handle, width, height) = try_start_scanning(&libsane)?;

	// 10 MB buf for the image
	let mut image = Vec::<u8>::with_capacity(100 * 1024 * 1024);

	let mut buf = Vec::<u8>::with_capacity(1024 * 1024);
	let mut bytes_written = 0_i32;
	loop {
		unsafe {
			buf.set_len(0);
		}
		let status = unsafe {
			libsane.sane_read(
				device_handle.0,
				buf.as_mut_ptr(),
				buf.capacity() as i32,
				&mut bytes_written as *mut i32,
			)
		};
		match status {
			sane::SANE_Status_SANE_STATUS_EOF => {
				debug!("EOF ({} bytes read).", bytes_written);
				break;
			}
			sane::SANE_Status_SANE_STATUS_GOOD => {
				debug!("Good ({} bytes read).", bytes_written);
			}
			status => {
				bail!("Failed to read data {}.", status);
			}
		}

		unsafe {
			buf.set_len(bytes_written as usize);
		}
		image.extend_from_slice(&buf);
	}

	unsafe {
		libsane.sane_cancel(device_handle.0);
	}

	Ok(ScanImage {
		raw_data: image,
		width,
		height,
	})
}

fn init_libsane() -> anyhow::Result<sane::libsane> {
	let libsane = unsafe { sane::libsane::new("libsane.so.1")? };

	let build_ver_major = 1;
	let build_ver_minor = 0;
	let build_revision = 0;
	let mut version_code: i32 =
		build_ver_major << 24 | build_ver_minor << 16 | build_revision & 0xffff;

	let status: sane::SANE_Status =
		unsafe { libsane.sane_init(&mut version_code as *mut sane::SANE_Int, None) };
	if status != sane::SANE_Status_SANE_STATUS_GOOD {
		bail!("Sane status is not good: {}", status);
	}
	Ok(libsane)
}

fn try_start_scanning<'a>(libsane: &'a sane::libsane) -> anyhow::Result<(Device<'a>, u32, u32)> {
	info!("Fetching printers.");
	let mut device_list: *mut *const sane::SANE_Device = std::ptr::null_mut();
	let status: sane::SANE_Status = unsafe {
		libsane.sane_get_devices(&mut device_list as *mut *mut *const sane::SANE_Device, 1)
	};
	if status != sane::SANE_Status_SANE_STATUS_GOOD {
		bail!("Failed to get devices {}", status);
	}
	let device_count = unsafe {
		let mut device_count = 0_usize;
		while !(*device_list.add(device_count)).is_null() {
			device_count += 1;
		}
		device_count
	};
	debug!("Number of devices found: {}.", device_count);
	let device_list: &[*const sane::SANE_Device] =
		unsafe { std::slice::from_raw_parts(device_list, device_count) };

	for (idx, device) in device_list.iter().copied().enumerate() {
		unsafe {
			let name = CStr::from_ptr((*device).name).to_string_lossy();
			let vendor = CStr::from_ptr((*device).vendor).to_string_lossy();
			let model = CStr::from_ptr((*device).model).to_string_lossy();
			let type_ = CStr::from_ptr((*device).type_).to_string_lossy();
			info!("{}. {}", idx + 1, model);
			info!("\tVendor: {}", vendor);
			info!("\tName: {}", name);
			info!("\tType: {}", type_);
		}
	}

	if device_list.is_empty() {
		bail!("No scanners found.");
	}

	let mut device_handle: sane::SANE_Handle = std::ptr::null_mut();
	let status: sane::SANE_Status = unsafe {
		libsane.sane_open(
			(*device_list[0]).name,
			&mut device_handle as *mut sane::SANE_Handle,
		)
	};
	if status != sane::SANE_Status_SANE_STATUS_GOOD {
		bail!("Failed to open device {}.", status);
	}
	let device_handle = unsafe { Device::from_raw_handle(device_handle, &libsane) };

	let mut option_count_value = 0_i32;
	let status: sane::SANE_Status = unsafe {
		libsane.sane_control_option(
			device_handle.0,
			0,
			sane::SANE_Action_SANE_ACTION_GET_VALUE,
			(&mut option_count_value as *mut i32).cast::<c_void>(),
			std::ptr::null_mut(),
		)
	};
	if status != sane::SANE_Status_SANE_STATUS_GOOD {
		bail!("Failed to get control option (option count) {}", status);
	}
	debug!("Number of available options: {}.", option_count_value);

	for option_num in 1..option_count_value {
		let option_descriptor =
			unsafe { libsane.sane_get_option_descriptor(device_handle.0, option_num) };
		if option_descriptor.is_null() {
			bail!("Failed to get option count");
		}

		unsafe {
			info!(
				"{:?}",
				CStr::from_ptr((*option_descriptor).title).to_string_lossy()
			);
			info!(
				"\t{:?}",
				CStr::from_ptr((*option_descriptor).name).to_string_lossy()
			);
			info!(
				"\t{:?}",
				CStr::from_ptr((*option_descriptor).desc).to_string_lossy()
			);
			info!(
				"Type {:?}. Unit {:?}. Size {:?}. Cap {:?}. Constraint type {:?}",
				(*option_descriptor).type_,
				(*option_descriptor).unit,
				(*option_descriptor).size,
				(*option_descriptor).cap,
				(*option_descriptor).constraint_type
			);
			match (*option_descriptor).constraint_type {
				sane::SANE_Constraint_Type_SANE_CONSTRAINT_NONE => (),
				sane::SANE_Constraint_Type_SANE_CONSTRAINT_RANGE => {
					let range = (*option_descriptor).constraint.range;
					info!(
						"\tRange: {}-{}. Quant {}. Steps {}.",
						(*range).min,
						(*range).max,
						(*range).quant,
						(*range).max / (*range).quant,
					);
				}
				sane::SANE_Constraint_Type_SANE_CONSTRAINT_WORD_LIST => {
					let word_list = (*option_descriptor).constraint.word_list;
					let list_len = *word_list;
					let word_list_slice =
						std::slice::from_raw_parts(word_list.add(1), list_len as usize);
					info!("\tPossible values: {:?}", word_list_slice);
				}
				sane::SANE_Constraint_Type_SANE_CONSTRAINT_STRING_LIST => {
					let string_list = (*option_descriptor).constraint.string_list;
					let mut list_len = 0;
					while !(*(string_list.add(list_len))).is_null() {
						list_len += 1;
					}
					let string_list_vec = std::slice::from_raw_parts(string_list, list_len)
						.iter()
						.map(|&str_ptr| CStr::from_ptr(str_ptr).to_string_lossy())
						.collect::<Vec<_>>();
					info!("\tPossible values: {:?}", string_list_vec);
				}
				_ => bail!("Unknown constraint type"),
			}
		}

		unsafe {
			let name = CStr::from_ptr((*option_descriptor).name);
			if name == CStr::from_bytes_with_nul_unchecked(b"resolution\0") {
				info!("Setting resolution");

				let mut value = match (*option_descriptor).type_ {
					sane::SANE_Value_Type_SANE_TYPE_INT | sane::SANE_Value_Type_SANE_TYPE_FIXED => {
						// 300 DPI
						300_i32
					}
					type_ => {
						bail!(
							"Resolution type should only be either int or fixed. It's {}.",
							type_
						);
					}
				};

				info!("Setting scan resolution to 300 dpi.");
				let mut additional_status: sane::SANE_Int = 0;
				let status: sane::SANE_Status = libsane.sane_control_option(
					device_handle.0,
					option_num,
					sane::SANE_Action_SANE_ACTION_SET_VALUE,
					(&mut value as *mut i32).cast::<c_void>(),
					&mut additional_status as *mut sane::SANE_Int,
				);
				if status != sane::SANE_Status_SANE_STATUS_GOOD {
					bail!(
						"Failed to get control option (no {}) {}",
						option_num,
						status
					);
				}
				info!("Additional status: {:b}.", additional_status);

				let mut dpi = 0_i32;
				let status: sane::SANE_Status = libsane.sane_control_option(
					device_handle.0,
					option_num,
					sane::SANE_Action_SANE_ACTION_GET_VALUE,
					(&mut dpi as *mut i32).cast::<c_void>(),
					std::ptr::null_mut(),
				);
				if status != sane::SANE_Status_SANE_STATUS_GOOD {
					bail!("Failed to fetch back set dpi setting {}.", status);
				}
				info!("DPI set to {}.", dpi);
			} else if name == CStr::from_bytes_with_nul_unchecked(b"tl-x\0") {
				// 2 3 4 5 1
				let mut value = 0_i32;

				let status: sane::SANE_Status = libsane.sane_control_option(
					device_handle.0,
					option_num,
					sane::SANE_Action_SANE_ACTION_GET_VALUE,
					(&mut value as *mut i32).cast::<c_void>(),
					std::ptr::null_mut(),
				);
				if status != sane::SANE_Status_SANE_STATUS_GOOD {
					bail!(
						"Failed to get control option (no {}) {}",
						option_num,
						status
					);
				}
				info!("tl-x: {}", value);
			} else if name == CStr::from_bytes_with_nul_unchecked(b"tl-y\0") {
				let mut value = 0_i32;

				let status: sane::SANE_Status = libsane.sane_control_option(
					device_handle.0,
					option_num,
					sane::SANE_Action_SANE_ACTION_GET_VALUE,
					(&mut value as *mut i32).cast::<c_void>(),
					std::ptr::null_mut(),
				);
				if status != sane::SANE_Status_SANE_STATUS_GOOD {
					bail!(
						"Failed to get control option (no {}) {}",
						option_num,
						status
					);
				}
				info!("tl-y: {}", value);
			} else if name == CStr::from_bytes_with_nul_unchecked(b"br-x\0") {
				let mut value = 0_i32;

				let status: sane::SANE_Status = libsane.sane_control_option(
					device_handle.0,
					option_num,
					sane::SANE_Action_SANE_ACTION_GET_VALUE,
					(&mut value as *mut i32).cast::<c_void>(),
					std::ptr::null_mut(),
				);
				if status != sane::SANE_Status_SANE_STATUS_GOOD {
					bail!(
						"Failed to get control option (no {}) {}",
						option_num,
						status
					);
				}
				info!("br-x: {}", value);
			} else if name == CStr::from_bytes_with_nul_unchecked(b"br-y\0") {
				let mut value = 0_i32;

				let status: sane::SANE_Status = libsane.sane_control_option(
					device_handle.0,
					option_num,
					sane::SANE_Action_SANE_ACTION_GET_VALUE,
					(&mut value as *mut i32).cast::<c_void>(),
					std::ptr::null_mut(),
				);
				if status != sane::SANE_Status_SANE_STATUS_GOOD {
					bail!(
						"Failed to get control option (no {}) {}",
						option_num,
						status
					);
				}
				info!("br-y: {}", value);
			}
		}
	}

	let status = unsafe { libsane.sane_start(device_handle.0) };
	if status != sane::SANE_Status_SANE_STATUS_GOOD {
		bail!("Failed to initiate image acquisition {}.", status);
	}

	let mut sane_parameters = sane::SANE_Parameters {
		format: 0,
		last_frame: 0,
		bytes_per_line: 0,
		pixels_per_line: 0,
		lines: 0,
		depth: 0,
	};

	let status = unsafe {
		libsane.sane_get_parameters(
			device_handle.0,
			&mut sane_parameters as *mut sane::SANE_Parameters,
		)
	};
	if status != sane::SANE_Status_SANE_STATUS_GOOD {
		bail!("Failed to get scan parameters {}.", status);
	}

	info!("Print parameters:");
	info!("\tFormat {}.", sane_parameters.format);
	info!("\tLast Frame {}.", sane_parameters.last_frame);
	info!("\tBytes per line {}.", sane_parameters.bytes_per_line);
	info!("\tPixels per line {}.", sane_parameters.pixels_per_line);
	info!("\tLines {}.", sane_parameters.lines);
	info!("\tDepth {}.", sane_parameters.depth);

	let width = sane_parameters.pixels_per_line as u32;
	let height = sane_parameters.lines as u32;

	info!("Width: {}.", width);
	info!("Height: {}.", height);
	Ok((device_handle, width, height))
}

async fn scan_stream_bmp() -> StreamingReceiver<Result<Bytes, anyhow::Error>> {
	let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<Result<Bytes, anyhow::Error>>();

	tokio::task::spawn_blocking(move || {
		let libsane = match init_libsane() {
			Ok(v) => v,
			Err(e) => {
				sender.send(Err(e)).unwrap();
				return;
			}
		};
		let (device_handle, width, height) = match try_start_scanning(&libsane) {
			Ok(v) => v,
			Err(e) => {
				sender.send(Err(e)).unwrap();
				return;
			}
		};
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
		if let Err(e) = encode_as_bmp(width * height * 3, (width, height), &mut bmp_header) {
			sender.send(Err(anyhow::Error::from(e))).unwrap();
			return;
		}
		sender.send(Ok(Bytes::from(bmp_header))).unwrap();

		// Vector capacity must be divisible by 3, so that we always get full pixels. Otherwise, we'd
		// get color artifacts when switching from RGB to BGR
		let mut buf = Vec::<u8>::with_capacity(3 * 1024 * 1024);
		let mut bytes_written = 0_i32;

		loop {
			unsafe {
				buf.set_len(0);
			}
			let status = unsafe {
				libsane.sane_read(
					device_handle.0,
					buf.as_mut_ptr(),
					buf.capacity() as i32,
					&mut bytes_written as *mut i32,
				)
			};
			match status {
				sane::SANE_Status_SANE_STATUS_EOF => {
					debug!("EOF ({} bytes read).", bytes_written);
					break;
				}
				sane::SANE_Status_SANE_STATUS_GOOD => {
					debug!("Good ({} bytes read).", bytes_written);
				}
				status => {
					sender
						.send(Err(anyhow!("Failed to read data {}.", status)))
						.unwrap();
					break;
				}
			}

			unsafe {
				buf.set_len(bytes_written as usize);
			}
			let mut cloned_buf = buf.clone();
			rgb_to_bgr(&mut cloned_buf);
			sender.send(Ok(Bytes::from(cloned_buf))).unwrap();
		}

		unsafe {
			libsane.sane_cancel(device_handle.0);
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
