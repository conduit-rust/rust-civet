#![warn(rust_2018_idioms)]

extern crate civet_sys as _;
extern crate conduit;
extern crate libc;

use std::io::prelude::*;
use std::io::{self, BufWriter};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use conduit::{header, Extensions, Handler, HeaderMap, Host, Method, Scheme, TypeMap, Version};

use raw::{get_header, get_headers, get_request_info};
use raw::{Header, RequestInfo};

pub use config::Config;

mod config;
mod raw;

pub struct Connection<'a> {
    request: CivetRequest<'a>,
    written: bool,
}

pub struct CivetRequest<'a> {
    conn: &'a raw::Connection,
    request_info: RequestInfo<'a>,
    headers: HeaderMap,
    extensions: Extensions,
    version: Version,
    method: Method,
}

impl<'a> conduit::RequestExt for CivetRequest<'a> {
    fn http_version(&self) -> Version {
        self.version
    }

    fn method(&self) -> &Method {
        &self.method
    }

    fn scheme(&self) -> Scheme {
        if self.request_info.is_ssl() {
            Scheme::Https
        } else {
            Scheme::Http
        }
    }

    fn host(&self) -> Host<'_> {
        Host::Name(get_header(self.conn, header::HOST).unwrap())
    }

    fn virtual_root(&self) -> Option<&str> {
        None
    }

    fn path(&self) -> &str {
        self.request_info.url().unwrap()
    }

    fn query_string(&self) -> Option<&str> {
        self.request_info.query_string()
    }

    fn remote_addr(&self) -> SocketAddr {
        let ip = self.request_info.remote_ip();
        let ip = Ipv4Addr::new(
            (ip >> 24) as u8,
            (ip >> 16) as u8,
            (ip >> 8) as u8,
            (ip >> 0) as u8,
        );
        SocketAddr::V4(SocketAddrV4::new(ip, self.request_info.remote_port()))
    }

    fn content_length(&self) -> Option<u64> {
        get_header(self.conn, header::CONTENT_LENGTH).and_then(|s| s.parse().ok())
    }

    fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    fn body(&mut self) -> &mut dyn Read {
        self
    }

    fn extensions(&self) -> &Extensions {
        &self.extensions
    }

    fn mut_extensions(&mut self) -> &mut Extensions {
        &mut self.extensions
    }
}

impl<'a> Connection<'a> {
    fn new(conn: &raw::Connection) -> Result<Connection<'_>, String> {
        match request_info(conn) {
            Ok(info) => {
                let mut headers = HeaderMap::new();
                for (name, value) in HeaderIterator::new(conn) {
                    headers.insert(
                        header::HeaderName::from_bytes(name.as_bytes())
                            .map_err(|e| e.to_string())?,
                        header::HeaderValue::from_bytes(value.as_bytes())
                            .map_err(|e| e.to_string())?,
                    );
                }
                let method = Method::from_bytes(info.method().unwrap().as_bytes())
                    .expect("Bad request method"); // FIXME: unwrap and expect panic

                let version = match info.http_version().unwrap() {
                    "1.0" => Version::HTTP_10,
                    "1.1" => Version::HTTP_11,
                    _ => Version::default(),
                };

                let request = CivetRequest {
                    conn,
                    request_info: info,
                    headers,
                    extensions: TypeMap::new(),
                    method,
                    version,
                };

                Ok(Connection {
                    request,
                    written: false,
                })
            }
            Err(err) => Err(err),
        }
    }
}

impl<'a> Write for Connection<'a> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.written = true;
        match raw::write(self.request.conn, buf) {
            n if n < 0 => Err(io::Error::new(
                io::ErrorKind::Other,
                &format!("write error ({})", n)[..],
            )),
            n => Ok(n as usize),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> Read for CivetRequest<'a> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match raw::read(self.conn, buf) {
            n if n < 0 => Err(io::Error::new(
                io::ErrorKind::Other,
                &format!("read error ({})", n)[..],
            )),
            n => Ok(n as usize),
        }
    }
}

impl<'a> Drop for Connection<'a> {
    fn drop(&mut self) {
        if !self.written {
            let _ = write!(
                self,
                "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n"
            );
        }
    }
}

struct HeaderIterator<'a> {
    headers: Vec<Header<'a>>,
    position: usize,
}

impl<'a> HeaderIterator<'a> {
    fn new(conn: &raw::Connection) -> HeaderIterator<'_> {
        HeaderIterator {
            headers: get_headers(conn),
            position: 0,
        }
    }
}

impl<'a> Iterator for HeaderIterator<'a> {
    type Item = (&'a str, &'a str);
    fn next(&mut self) -> Option<Self::Item> {
        let pos = self.position;
        let headers = &self.headers;

        if self.headers.len() <= pos {
            None
        } else {
            let header = &headers[pos];
            self.position += 1;
            header.name().map(|name| (name, header.value().unwrap()))
        }
    }
}

pub struct Server(raw::Server<Box<dyn Handler + 'static + Sync>>);

impl Server {
    pub fn start<H: Handler + 'static + Sync>(options: Config, handler: H) -> io::Result<Server> {
        fn internal_handler(
            conn: &mut raw::Connection,
            handler: &Box<dyn Handler + 'static + Sync>,
        ) -> Result<(), ()> {
            let mut connection = Connection::new(conn).unwrap();
            let response = handler.call(&mut connection.request);
            let mut writer = BufWriter::new(connection);

            fn err<W: Write>(writer: &mut W) {
                let _ = write!(
                    writer,
                    "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n"
                );
            }

            let (head, mut body) = match response {
                Ok(r) => r,
                Err(_) => {
                    err(&mut writer);
                    return Err(());
                }
            }
            .into_parts();

            write!(
                &mut writer,
                "HTTP/1.1 {} {}\r\n",
                head.status.as_str(),
                head.status.canonical_reason().unwrap_or("UNKNOWN")
            )
            .map_err(|_| ())?;

            for (key, value) in head.headers.iter() {
                write!(&mut writer, "{}: ", *key).map_err(|_| ())?;
                writer.write(value.as_bytes()).map_err(|_| ())?;
                writer.write(b"\r\n").map_err(|_| ())?;
            }

            write!(&mut writer, "\r\n").map_err(|_| ())?;
            body.write_body(&mut writer).map_err(|_| ())?;

            Ok(())
        }

        let handler = Box::new(handler);
        let raw_callback = raw::ServerCallback::new(internal_handler, handler);
        Ok(Server(raw::Server::start(options, raw_callback)?))
    }
}

fn request_info(connection: &raw::Connection) -> Result<RequestInfo<'_>, String> {
    match get_request_info(connection) {
        Some(info) => Ok(info),
        None => Err("Couldn't get request info for connection".to_string()),
    }
}

#[cfg(test)]
mod test {
    use super::{Config, Server};
    use conduit::{box_error, Body, Handler, HandlerResult, HttpResult, RequestExt, Response};
    use std::io;
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{channel, Sender};
    use std::sync::Mutex;

    fn noop(_: &mut dyn RequestExt) -> HttpResult {
        unreachable!()
    }

    fn request(addr: SocketAddr, req: &str) -> String {
        use std::io::{Read, Write};

        let mut s = TcpStream::connect(&addr).unwrap();
        s.write_all(req.trim_start().as_bytes()).unwrap();
        let mut ret = String::new();
        s.read_to_string(&mut ret).unwrap();
        ret
    }

    fn port() -> u16 {
        static CNT: AtomicUsize = AtomicUsize::new(0);
        CNT.fetch_add(1, Ordering::SeqCst) as u16 + 13038
    }

    fn cfg(port: u16) -> Config {
        let mut cfg = Config::new();
        cfg.port(port).threads(1);
        return cfg;
    }

    #[test]
    fn smoke() {
        Server::start(cfg(port()), noop).unwrap();
    }

    #[test]
    fn dupe_port() {
        let port = port();
        let s1 = Server::start(cfg(port), noop);
        assert!(s1.is_ok());
        let s2 = Server::start(cfg(port), noop);
        assert!(s2.is_err());
    }

    #[test]
    fn drops_handler() {
        static mut DROPPED: bool = false;
        struct Foo;
        impl Handler for Foo {
            fn call(&self, _req: &mut dyn RequestExt) -> HandlerResult {
                panic!()
            }
        }
        impl Drop for Foo {
            fn drop(&mut self) {
                unsafe {
                    DROPPED = true;
                }
            }
        }

        drop(Server::start(cfg(port()), Foo));
        unsafe {
            assert!(DROPPED);
        }
    }

    #[test]
    fn invokes() {
        struct Foo(Mutex<Sender<()>>);
        impl Handler for Foo {
            fn call(&self, _req: &mut dyn RequestExt) -> HandlerResult {
                let Foo(ref tx) = *self;
                tx.lock().unwrap().send(()).unwrap();
                let body: Body = Box::new(io::empty());
                Response::builder().body(body).map_err(box_error)
            }
        }

        let (tx, rx) = channel();
        let handler = Foo(Mutex::new(tx));
        let port = port();
        let ip = Ipv4Addr::new(127, 0, 0, 1);
        let addr = SocketAddr::V4(SocketAddrV4::new(ip, port));
        let _s = Server::start(cfg(port), handler);
        request(
            addr,
            r"
GET / HTTP/1.1

",
        );
        rx.recv().unwrap();
    }

    #[test]
    fn header_sent() {
        struct Foo(Mutex<Sender<Vec<u8>>>);
        impl Handler for Foo {
            fn call(&self, req: &mut dyn RequestExt) -> HandlerResult {
                let Foo(ref tx) = *self;
                let mut header_val = Vec::new();
                header_val.extend_from_slice(req.headers().get("Foo").unwrap().as_bytes());
                tx.lock().unwrap().send(header_val).unwrap();
                Response::builder()
                    .body(Box::new(io::empty()) as Body)
                    .map_err(box_error)
            }
        }

        let (tx, rx) = channel();
        let handler = Foo(Mutex::new(tx));
        let port = port();
        let ip = Ipv4Addr::new(127, 0, 0, 1);
        let addr = SocketAddr::V4(SocketAddrV4::new(ip, port));
        let _s = Server::start(cfg(port), handler);
        request(
            addr,
            r"
GET / HTTP/1.1
Foo: bar

",
        );
        assert_eq!(rx.recv().unwrap(), b"bar");
    }

    #[test]
    fn failing_handler() {
        struct Foo;
        impl Handler for Foo {
            fn call(&self, _req: &mut dyn RequestExt) -> HandlerResult {
                panic!()
            }
        }

        let port = port();
        let ip = Ipv4Addr::new(127, 0, 0, 1);
        let addr = SocketAddr::V4(SocketAddrV4::new(ip, port));
        let _s = Server::start(cfg(port), Foo);
        request(
            addr,
            r"
GET / HTTP/1.1
Foo: bar

",
        );
    }

    #[test]
    fn failing_handler_is_500() {
        struct Foo;
        impl Handler for Foo {
            fn call(&self, _req: &mut dyn RequestExt) -> HandlerResult {
                panic!()
            }
        }

        let port = port();
        let ip = Ipv4Addr::new(127, 0, 0, 1);
        let addr = SocketAddr::V4(SocketAddrV4::new(ip, port));
        let _s = Server::start(cfg(port), Foo);
        let response = request(
            addr,
            r"
GET / HTTP/1.1
Foo: bar

",
        );
        assert!(
            response.contains("500 Internal"),
            "not a failing response: {}",
            response
        );
    }
}
