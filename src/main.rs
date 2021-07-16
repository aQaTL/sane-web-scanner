use anyhow::bail;
use log::{debug, info};
use std::ffi::CStr;

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
	debug!("gitara");

	let mut device_list: *mut *const sane::SANE_Device = std::ptr::null_mut();

	let status: sane::SANE_Status = unsafe {
		libsane.sane_get_devices(&mut device_list as *mut *mut *const sane::SANE_Device, 1)
	};
	if status != sane::SANE_Status_SANE_STATUS_GOOD {
		bail!("Failed to get devices {}", status);
	}
	let list_len = unsafe {
		let mut list_len = 0_isize;
		while !(*device_list.offset(list_len)).is_null() {
			list_len += 1;
		}
		list_len as usize
	};
	debug!("Number of devices found: {}", list_len);
	let device_list: &[*const sane::SANE_Device] =
		unsafe { std::slice::from_raw_parts(device_list, list_len) };

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
	debug!("gitara");

	Ok(())
}
