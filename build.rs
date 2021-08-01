use std::fs::{DirEntry, File};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

fn main() -> Result<(), std::io::Error> {
	println!("cargo:rerun-if-changed=frontend");

	let exit_status = Command::new("make").arg("export").spawn()?.wait()?;

	if !exit_status.success() {
		std::process::exit(exit_status.code().unwrap());
	}

	let root = "frontend/out";
	let frontend_files = DirIter::new(PathBuf::from(root))?
		.filter_map(|e| e.ok())
		.filter(|e| e.path().is_file())
		.map(|e| e.path().to_string_lossy().replace("\\", "/"))
		.map(|filename| {
			format!(
				"(\"{}\", &include_bytes!(\"{}/{}\")[..]), ",
				filename
					.trim_start_matches(&format!("{}/", root))
					.to_owned(),
				env!("CARGO_MANIFEST_DIR").replace("\\", "/"),
				filename,
			)
		})
		.collect::<String>();

	let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());

	let mut frontend_files_file = File::create(out_dir.join("frontend_files.array"))?;
	frontend_files_file.write_all(b"[")?;
	frontend_files_file.write_all(frontend_files.as_bytes())?;
	frontend_files_file.write_all(b"]")?;

	Ok(())
}

struct DirIter {
	stack: Vec<Result<DirEntry, std::io::Error>>,
}

impl DirIter {
	pub fn new(root: PathBuf) -> Result<Self, std::io::Error> {
		let stack = std::fs::read_dir(&root)?.collect::<Vec<_>>();
		Ok(DirIter { stack })
	}
}

impl Iterator for DirIter {
	type Item = Result<DirEntry, std::io::Error>;

	fn next(&mut self) -> Option<Self::Item> {
		loop {
			match self.stack.pop() {
				Some(Ok(item)) if item.path().is_dir() => {
					let stack_iter = match std::fs::read_dir(item.path()) {
						Ok(stack_iter) => stack_iter,
						Err(err) => return Some(Err(err)),
					};
					self.stack.extend(stack_iter);
				}
				item @ Some(Ok(_)) => return item,
				Some(Err(err)) => return Some(Err(err)),
				None => return None,
			};
		}
	}
}
