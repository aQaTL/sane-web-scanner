use anyhow::bail;
use log::debug;

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
		let mut device_list: *mut *const sane::SANE_Device = device_list;
		let mut list_len = 0_isize;
		while !(*device_list).is_null() {
			list_len += 1;
			device_list = device_list.add(1);
		}
		list_len as usize
	};
	debug!("Number of devices found: {}", list_len);
	let device_list: &[*const sane::SANE_Device] =
		unsafe { std::slice::from_raw_parts(device_list, list_len) };
	debug!("gitara");

	Ok(())
}
