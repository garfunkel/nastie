#![feature(proc_macro_hygiene, decl_macro)]

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::{thread, time};

use base64;
use clap::{App, Arg};
use handlebars::Handlebars;
use reqwest::{blocking::Client, header};
use serde::Serialize;

#[macro_use]
extern crate rocket;
use rocket::http::{ContentType, Status};
use rocket::{response, Config};

#[macro_use]
extern crate rust_embed;

#[macro_use]
extern crate serde_json;

const DEFAULT_HOST: &str = "localhost";
const DEFAULT_PORT: &str = "80";
const DEFAULT_BIND_HOST: &str = DEFAULT_HOST;
const DEFAULT_BIND_PORT: &str = "8000";
const DEFAULT_WEB_UI_USER: &str = "root";
const API_URL_BASE: &str = "/api/v2.0/";
const TEMPLATE_INDEX: &str = "index.html";
const ICON_FREEBSD: &str = "/static/icons/beastie.png";

#[derive(RustEmbed)]
#[folder = "static/"]
struct Static;

#[derive(RustEmbed)]
#[folder = "templates/"]
struct Templates;

#[derive(Serialize)]
struct Jail {
	jail_ip: String,
	admin_url: Option<String>,
	icon_url: Option<String>,
}

fn list(client: &Client, api_url_base: &String) -> std::collections::HashMap<String, Jail> {
	let mut jails = HashMap::new();
	let response = client
		.get(&(api_url_base.to_owned() + "jail"))
		.send()
		.unwrap();

	let obj = json::parse(&response.text().unwrap()).unwrap();

	for jail_obj in obj.members() {
		jails.insert(
			jail_obj["id"].to_string(),
			Jail {
				jail_ip: jail_obj["ip4_addr"].to_string(),
				admin_url: None,
				icon_url: None,
			},
		);
	}

	let response = client
		.get(&(api_url_base.to_owned() + "plugin"))
		.send()
		.unwrap();

	let obj = json::parse(&response.text().unwrap()).unwrap();

	for plugin_obj in obj.members() {
		let name = plugin_obj["name"].to_string();

		match &plugin_obj["admin_portals"] {
			json::JsonValue::Array(admin_urls) => {
				jails.entry(name.clone()).and_modify(|jail| {
					jail.admin_url = Some(admin_urls[0].to_string());
					jail.icon_url = Some(
						plugin_obj["plugin_repository"]
							.to_string()
							.trim_end_matches(".git")
							.replace("github.com", "raw.githubusercontent.com")
							+ &format!(
								"/master/icons/{}.png",
								name.replace("plexmediaserver", "plex")
							),
					);
				});
			}
			_ => (),
		}
	}

	for (_, jail) in jails.iter_mut() {
		if jail.icon_url == None {
			jail.icon_url = Some(ICON_FREEBSD.into());
		}
	}

	jails
}

#[get("/")]
fn index(
	handlebars: rocket::State<Handlebars>,
	arc_jails: rocket::State<Arc<RwLock<HashMap<String, Jail>>>>,
) -> rocket::response::content::Html<String> {
	let jails = arc_jails.read().unwrap();

	rocket::response::content::Html(handlebars.render(TEMPLATE_INDEX, &json!(&*jails)).unwrap())
}

#[get("/static/<path..>")]
fn static_file<'r>(path: std::path::PathBuf) -> response::Result<'r> {
	let filename = path.display().to_string();

	Static::get(&filename).map_or_else(
		|| Err(Status::NotFound),
		|d| {
			let ext = path
				.as_path()
				.extension()
				.and_then(std::ffi::OsStr::to_str)
				.ok_or_else(|| Status::new(400, "Could not get file extension"))?;
			let content_type = ContentType::from_extension(ext)
				.ok_or_else(|| Status::new(400, "Could not get file content type"))?;
			response::Response::build()
				.header(content_type)
				.sized_body(std::io::Cursor::new(d))
				.ok()
		},
	)
}

fn main() {
	let matches = App::new(env!("CARGO_PKG_NAME"))
		.version(env!("CARGO_PKG_VERSION"))
		.about(env!("CARGO_PKG_DESCRIPTION"))
		.author(env!("CARGO_PKG_AUTHORS"))
		.arg(
			Arg::with_name("host")
				.help("FreeNAS/TrueNAS host")
				.default_value(DEFAULT_HOST),
		)
		.arg(
			Arg::with_name("port")
				.help("FreeNAS/TrueNAS port")
				.default_value(DEFAULT_PORT),
		)
		.arg(
			Arg::with_name("user")
				.short("u")
				.long("user")
				.help("Web UI root user")
				.default_value(DEFAULT_WEB_UI_USER),
		)
		.arg(
			Arg::with_name("password")
				.short("P")
				.long("password")
				.help("Web UI root password")
				.takes_value(true)
				.required(true),
		)
		.arg(
			Arg::with_name("bind-host")
				.short("H")
				.long("bind-host")
				.help("IP address to bind to")
				.default_value(DEFAULT_BIND_HOST),
		)
		.arg(
			Arg::with_name("bind-port")
				.short("p")
				.long("bind-port")
				.help("Port to bind to")
				.default_value(DEFAULT_BIND_PORT),
		)
		.arg(
			Arg::with_name("secure")
				.short("-s")
				.long("secure")
				.help("Connect using HTTPS"),
		)
		.get_matches();

	let host = matches.value_of("host").unwrap();
	let port = matches.value_of("port").unwrap();
	let bind_host = matches.value_of("bind-host").unwrap();
	let bind_port = matches.value_of("bind-port").unwrap();
	let user = matches.value_of("user").unwrap();
	let password = matches.value_of("password").unwrap();

	let protocol = match matches.is_present("secure") {
		true => "https",
		_ => "http",
	};

	let auth_value = format!("Basic {}", base64::encode(format!("{}:{}", user, password)));
	let api_url_base = format!("{}://{}:{}{}", protocol, host, port, API_URL_BASE);
	let jails: HashMap<String, Jail> = HashMap::new();
	let arc_jails = Arc::new(RwLock::new(jails));
	let arc2_jails = arc_jails.clone();

	thread::spawn(move || {
		let mut headers = header::HeaderMap::new();
		headers.insert(
			header::AUTHORIZATION,
			header::HeaderValue::from_str(&auth_value).unwrap(),
		);

		let client = Client::builder().default_headers(headers).build().unwrap();

		loop {
			let mut jails = arc2_jails.write().unwrap();
			*jails = list(&client, &api_url_base);

			std::mem::drop(jails);

			thread::sleep(time::Duration::from_secs(30));
		}
	});

	let env = rocket::config::Environment::active().unwrap();
	let mut handlebars = Handlebars::new();
	let template = Templates::get(TEMPLATE_INDEX).unwrap();

	handlebars
		.register_template_string(TEMPLATE_INDEX, std::str::from_utf8(&template).unwrap())
		.unwrap();

	rocket::custom(
		Config::build(env)
			.address(bind_host)
			.port(bind_port.parse().unwrap())
			.finalize()
			.unwrap(),
	)
	.manage(handlebars)
	.manage(arc_jails)
	.mount("/", routes![static_file, index])
	.launch();
}
