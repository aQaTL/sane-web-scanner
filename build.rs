use std::path::PathBuf;

fn main() {
	let bindings = bindgen::builder()
		.header("/usr/include/sane/sane.h")
		.dynamic_library_name("libsane")
		.generate()
		.unwrap();

	bindings
		.write_to_file(
			[std::env::var("OUT_DIR").unwrap().as_str(), "sane.rs"]
				.iter()
				.collect::<PathBuf>(),
		)
		.unwrap();
}
