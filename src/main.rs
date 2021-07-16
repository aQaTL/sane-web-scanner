use anyhow::bail;
use log::{debug, info};
use std::ffi::{c_void, CStr};

mod sane {
	#![allow(non_upper_case_globals)]
	#![allow(non_camel_case_types)]
	#![allow(non_snake_case)]
	#![allow(dead_code)]

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

fn main() -> anyhow::Result<()> {
	flexi_logger::Logger::try_with_str(log_str())?.start()?;

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
		return Ok(());
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

	let option_count_descriptor = unsafe { libsane.sane_get_option_descriptor(device_handle.0, 0) };
	if option_count_descriptor.is_null() {
		bail!("Failed to get option count");
	}

	unsafe {
		info!(
			"{:?}",
			CStr::from_ptr((*option_count_descriptor).name).to_string_lossy()
		);
		info!(
			"{:?}",
			CStr::from_ptr((*option_count_descriptor).title).to_string_lossy()
		);
		info!(
			"{:?}",
			CStr::from_ptr((*option_count_descriptor).desc).to_string_lossy()
		);
		info!("{:?}", (*option_count_descriptor).type_);
		info!("{:?}", (*option_count_descriptor).unit);
		info!("{:?}", (*option_count_descriptor).size);
		info!("{:?}", (*option_count_descriptor).cap);
		info!("{:?}", (*option_count_descriptor).constraint_type);
	}

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
	println!("Number of available options: {}.", option_count_value);

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
			info!("\t{:?}", (*option_descriptor).type_);
			info!("\t{:?}", (*option_descriptor).unit);
			info!("\t{:?}", (*option_descriptor).size);
			info!("\t{:?}", (*option_descriptor).cap);
			info!("\t{:?}", (*option_descriptor).constraint_type);
			match (*option_descriptor).constraint_type {
				sane::SANE_Constraint_Type_SANE_CONSTRAINT_NONE => (),
				sane::SANE_Constraint_Type_SANE_CONSTRAINT_RANGE => {
					let range = (*option_descriptor).constraint.range;
					info!(
						"\tRange: {}-{}. Quant {}.",
						(*range).min,
						(*range).max,
						(*range).quant
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
						.into_iter()
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
	info!("\tLast Frame {}.", sane_parameters.format);
	info!("\tBytes per line {}.", sane_parameters.format);
	info!("\tPixels per line {}.", sane_parameters.format);
	info!("\tLines {}.", sane_parameters.format);
	info!("\tDepth {}.", sane_parameters.format);

	// 10 MB buf for the image
	let mut image = Vec::<u8>::with_capacity(100 * 1024 * 1024);

	let mut buf = Vec::<u8>::with_capacity(1024 * 1024);
	let mut bytes_written = 0_i32;
	loop {
		// let mut sane_parameters = sane::SANE_Parameters {
		// 	format: 0,
		// 	last_frame: 0,
		// 	bytes_per_line: 0,
		// 	pixels_per_line: 0,
		// 	lines: 0,
		// 	depth: 0,
		// };
		// let status = unsafe {
		// 	libsane.sane_get_parameters(
		// 		device_handle.0,
		// 		&mut sane_parameters as *mut sane::SANE_Parameters,
		// 	)
		// };
		// if status != sane::SANE_Status_SANE_STATUS_GOOD {
		// 	bail!("Failed to get scan parameters {}.", status);
		// }

		// info!("Print parameters:");
		// info!("\tFormat {}.", sane_parameters.format);
		// info!("\tLast Frame {}.", sane_parameters.format);
		// info!("\tBytes per line {}.", sane_parameters.format);
		// info!("\tPixels per line {}.", sane_parameters.format);
		// info!("\tLines {}.", sane_parameters.format);
		// info!("\tDepth {}.", sane_parameters.format);

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
	info!("\tLast Frame {}.", sane_parameters.format);
	info!("\tBytes per line {}.", sane_parameters.format);
	info!("\tPixels per line {}.", sane_parameters.format);
	info!("\tLines {}.", sane_parameters.format);
	info!("\tDepth {}.", sane_parameters.format);

	unsafe {
		libsane.sane_cancel(device_handle.0);
	}

	info!("Scan completed. Saving to file.");
	std::fs::write("./scanned_document", image.as_slice())?;

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
