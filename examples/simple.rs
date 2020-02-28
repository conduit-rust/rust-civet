extern crate civet;
extern crate conduit;

use std::io::prelude::*;
use std::io::{self, Cursor};
use std::sync::mpsc::channel;

use civet::{Config, Server};
use conduit::{header, vec_to_body, Body, RequestExt, Response};

macro_rules! http_write {
    ($dst:expr, $fmt:expr) => (
        write!(&mut $dst, concat!($fmt, "\r\n"))?
    );
    ($dst:expr, $fmt:expr, $($arg:tt)*) => (
        write!(&mut $dst, concat!($fmt, "\r\n"), $($arg)*)?
    )
}

fn main() {
    let _a = Server::start(Config::new(), handler);
    let (_tx, rx) = channel::<()>();
    rx.recv().unwrap();
}

fn handler(req: &mut dyn RequestExt) -> io::Result<Response<Body>> {
    let mut res = Cursor::new(Vec::with_capacity(10000));

    http_write!(res, "<style>body {{ font-family: sans-serif; }}</style>");
    http_write!(res, "<p>HTTP {:?}</p>", req.http_version());
    http_write!(res, "<p>Method: {:?}</p>", req.method());
    http_write!(res, "<p>Scheme: {:?}</p>", req.scheme());
    http_write!(res, "<p>Host: {:?}</p>", req.host());
    http_write!(res, "<p>Path: {}</p>", req.path());
    http_write!(res, "<p>Query String: {:?}</p>", req.query_string());
    http_write!(res, "<p>Remote address: {}</p>", req.remote_addr());
    http_write!(res, "<p>Content Length: {:?}</p>", req.content_length());

    let mut body = String::new();
    req.body().read_to_string(&mut body).unwrap();
    http_write!(res, "<p>Input: {}", body);

    http_write!(res, "<h2>Headers</h2><ul>");

    for (key, value) in req.headers().iter() {
        http_write!(
            res,
            "<li>{} = {}</li>",
            key,
            value.to_str().unwrap_or_default()
        );
    }

    http_write!(res, "</ul>");

    let body: Body = vec_to_body(res.into_inner());
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "text/plain")
        .body(body)
        .unwrap())
}
