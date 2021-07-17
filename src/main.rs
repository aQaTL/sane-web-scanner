use std::ffi::{c_void, CStr};
use std::io::Write;
use std::path::Path;

use actix_web::{get, App, HttpResponse, HttpServer};
use anyhow::bail;
use log::{debug, error, info, warn};
use systemd_socket_activation::systemd_socket_activation;

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
				self.sane_exit();
			}
		}
	}
}

const fn log_str() -> &'static str {
	if cfg!(debug_assertions) {
		"debug"
	} else {
		"info"
	}
}

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
	flexi_logger::Logger::try_with_str(log_str())?.start()?;

	let mut http_server = HttpServer::new(|| App::new().service(scan_service));

	let (address, port) = ("0.0.0.0", 8000_u16);

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

#[get("/scan")]
async fn scan_service() -> HttpResponse {
	// TODO(aQaTL): Stream the data
	//  Possibly via a websocket?
	let mut image = scan().unwrap();

	info!("Scan completed. Saving to file.");

	// BMP is BGR by default, while our image is assumed to be RGB. This swaps red channel with blue.
	for chunk in image.raw_data.chunks_exact_mut(3) {
		chunk.swap(0, 2);
	}

	save_as_bmp(
		"./scanned_document.bmp".as_ref(),
		&image.raw_data,
		(image.width, image.height),
	)
	.unwrap();

	HttpResponse::Ok().body("git")
}

struct ScanImage {
	raw_data: Vec<u8>,
	width: u32,
	height: u32,
}

fn scan() -> anyhow::Result<ScanImage> {
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

fn save_as_bmp(path: &Path, img: &[u8], (width, height): (u32, u32)) -> Result<(), std::io::Error> {
	let mut file = std::fs::File::create(path)?;

	let file_header_size: u32 = 2 + 4 + 2 + 2 + 4;
	let image_header_size: u32 = 4 + 4 + 4 + 2 + 2 + 4 + 4 + 4 + 4 + 4 + 4;

	// file header
	file.write_all(&[b'B', b'M'])?;
	let file_size = file_header_size + image_header_size + img.len() as u32;
	file.write_all(&file_size.to_le_bytes())?;
	file.write_all(&[0, 0])?;
	file.write_all(&[0, 0])?;
	let pixel_data_offset = file_header_size + image_header_size;
	file.write_all(&pixel_data_offset.to_le_bytes())?;

	// image header
	file.write_all(&image_header_size.to_le_bytes())?;
	file.write_all(&width.to_le_bytes())?;
	file.write_all(&(-(height as i32)).to_le_bytes())?;
	file.write_all(&1_u16.to_le_bytes())?;
	file.write_all(&24_u16.to_le_bytes())?;
	file.write_all(&0_u32.to_le_bytes())?;
	file.write_all(&0_u32.to_le_bytes())?;
	file.write_all(&0_u32.to_le_bytes())?;
	file.write_all(&0_u32.to_le_bytes())?;
	file.write_all(&0_u32.to_le_bytes())?;
	file.write_all(&0_u32.to_le_bytes())?;

	file.write_all(img)?;

	Ok(())
}

struct Device<'a>(pub sane::SANE_Handle, &'a sane::libsane);

impl<'a> Device<'a> {
	pub unsafe fn from_raw_handle(handle: sane::SANE_Handle, libsane: &'a sane::libsane) -> Self {
		Device(handle, libsane)
	}
}

impl Drop for Device<'_> {
	fn drop(&mut self) {
		unsafe { self.1.sane_close(self.0) }
	}
}
