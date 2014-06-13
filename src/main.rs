#![feature(macro_rules)]

extern crate libc;
extern crate debug;
extern crate native;
extern crate collections;

use std::io::IoResult;
use civet::{Config,Server,Request,Response};

macro_rules! http_write(
    ($dst:expr, $fmt:expr $($arg:tt)*) => (
        try!(write!($dst, concat!($fmt, "\r\n") $($arg)*))
    )
)

mod civet;

fn main() {
    let _ = Server::start(Config { port: 8888, threads: 10 }, handler);

    loop {
        std::io::timer::sleep(1000);
    }
}

fn handler(req: &mut Request, res: &mut Response) -> IoResult<()> {
    http_write!(res, "HTTP/1.1 200 OK");
    http_write!(res, "Content-Type: text/html");
    http_write!(res, "");
    http_write!(res, "<p>Method: {}</p>", req.method());
    http_write!(res, "<p>URL: {}</p>", req.url());
    http_write!(res, "<p>HTTP: {}</p>", req.http_version());
    http_write!(res, "<p>Remote IP: {}</p>", req.remote_ip());
    http_write!(res, "<p>Remote User: {}</p>", req.remote_user());
    http_write!(res, "<p>Query String: {}</p>", req.query_string());
    http_write!(res, "<p>SSL?: {}</p>", req.is_ssl());
    http_write!(res, "<p>Header Count: {}</p>", req.count_headers());
    http_write!(res, "<p>User Agent: {}</p>", req.headers().find("User-Agent"));
    http_write!(res, "<p>Input: {}</p>", try!(req.read_to_str()));

    http_write!(res, "<h2>Headers</h2><ul>");

    for (key, value) in req.headers().iter() {
        http_write!(res, "<li>{} = {}</li>", key, value);
    }

    http_write!(res, "</ul>");

    Ok(())
}
