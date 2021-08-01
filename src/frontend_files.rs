use std::collections::HashMap;

pub type FrontendFiles = HashMap<&'static str, &'static [u8]>;

lazy_static::lazy_static! {
	pub static ref  FRONTEND_FILES: FrontendFiles =
		include!(concat!(env!("OUT_DIR"), "/frontend_files.array"))
			.iter()
			.cloned()
			.collect();
}
