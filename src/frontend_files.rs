use actix_web::dev::{AnyBody, HttpServiceFactory};
use actix_web::http::StatusCode;
use actix_web::web::Bytes;
use actix_web::{HttpRequest, HttpResponse, ResponseError};
use log::debug;
use std::collections::HashMap;
use std::fmt::Display;

pub type FrontendFiles = HashMap<&'static str, &'static [u8]>;

lazy_static::lazy_static! {
	pub static ref  FRONTEND_FILES: FrontendFiles =
		include!(concat!(env!("OUT_DIR"), "/frontend_files.array"))
			.iter()
			.cloned()
			.collect();
}

#[derive(Copy, Clone)]
pub struct Service;

impl HttpServiceFactory for Service {
	fn register(self, config: &mut actix_web::dev::AppService) {
		actix_web::Resource::new(["/", "/{resource..}"])
			.name("frontend_files::Service")
			.guard(actix_web::guard::Get())
			.to(serve_static_file)
			.register(config);
	}
}

async fn serve_static_file(
	http_req: HttpRequest,
) -> Result<HttpResponse, FrontendFilesServiceError> {
	let name = percent_encoding::percent_decode_str(http_req.uri().path())
		.decode_utf8()
		.map_err(|_| FrontendFilesServiceError::NotFound)?;

	let mut name = name.strip_prefix("/").unwrap_or(name.as_ref());

	if name == "" {
		name = "index.html"
	}

	debug!("Serving static file {:?}.", name);

	let file = FRONTEND_FILES
		.get(name)
		.ok_or(FrontendFilesServiceError::NotFound)?;
	Ok(HttpResponse::Ok().body(Bytes::from_static(*file)))
}

#[derive(Debug)]
enum FrontendFilesServiceError {
	NotFound,
}

impl Display for FrontendFilesServiceError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{:?}", self)
	}
}

impl ResponseError for FrontendFilesServiceError {
	fn status_code(&self) -> StatusCode {
		match self {
			FrontendFilesServiceError::NotFound => StatusCode::NOT_FOUND,
		}
	}

	fn error_response(&self) -> HttpResponse<AnyBody> {
		match self {
			FrontendFilesServiceError::NotFound => HttpResponse::NotFound().body(
				r####"<html>
<head>
    <title>404 Not Found</title>
<head>
<body>
    <center>
        <h1>404 Not Found</h1>
        <hr>
        <h3>Sane Web Scanner</h1>
    </center>
</body>
</html>"####,
			),
		}
	}
}
