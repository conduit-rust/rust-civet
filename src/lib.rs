#![crate_id="civet"]
#![crate_type="rlib"]
#![feature(unsafe_destructor)]

extern crate libc;
extern crate debug;
extern crate native;
extern crate collections;
extern crate semver;
extern crate conduit;

use std::io;
use std::io::net::ip::{IpAddr, Ipv4Addr};
use std::io::{IoResult, util};
use std::collections::HashMap;

use conduit::{Request, HeaderEntries, Handler};

use raw::{RequestInfo,Header};
use raw::{get_header,get_headers,get_request_info};
use status::{ToStatusCode};

pub use raw::Config;

mod raw;
pub mod status;

pub struct Connection<'a> {
    request: CivetRequest<'a>,
    written: bool,
}

pub struct CivetRequest<'a> {
    conn: &'a raw::Connection,
    request_info: RequestInfo<'a>,
    headers: Headers<'a>
}

fn ver(major: uint, minor: uint) -> semver::Version {
    semver::Version {
        major: major,
        minor: minor,
        patch: 0,
        pre: vec!(),
        build: vec!()
    }
}

impl<'a> conduit::Request for CivetRequest<'a> {
    fn http_version(&self) -> semver::Version {
        let version = self.request_info.http_version().unwrap();
        match version {
            "1.0" => ver(1, 0),
            "1.1" => ver(1, 1),
            _ => ver(1, 1)
        }
    }

    fn conduit_version(&self) -> semver::Version {
        ver(0, 1)
    }

    fn method<'a>(&'a self) -> conduit::Method<'a> {
        match self.request_info.method().unwrap() {
            "HEAD" => conduit::Head,
            "GET" => conduit::Get,
            "POST" => conduit::Post,
            "PUT" => conduit::Put,
            "DELETE" => conduit::Delete,
            "PATCH" => conduit::Patch,
            "PURGE" => conduit::Purge,
            "CONNECT" => conduit::Connect,
            "OPTIONS" => conduit::Options,
            "TRACE" => conduit::Trace,
            other @ _ => conduit::Other(other)
        }
    }

    fn scheme(&self) -> conduit::Scheme {
        if self.request_info.is_ssl() {
            conduit::Https
        } else {
            conduit::Http
        }
    }

    fn host<'a>(&'a self) -> conduit::Host<'a> {
        conduit::HostName(get_header(self.conn, "Host").unwrap())
    }

    fn virtual_root<'a>(&'a self) -> Option<&'a str> {
        None
    }

    fn path<'a>(&'a self) -> &'a str {
        self.request_info.url().unwrap()
    }

    fn query_string<'a>(&'a self) -> Option<&'a str> {
        self.request_info.query_string()
    }

    fn remote_ip(&self) -> IpAddr {
        let ip = self.request_info.remote_ip();
        Ipv4Addr((ip >> 24) as u8,
                 (ip >> 16) as u8,
                 (ip >>  8) as u8,
                 (ip >>  0) as u8)
    }

    fn content_length(&self) -> Option<uint> {
        get_header(self.conn, "Content-Length").and_then(from_str)
    }

    fn headers<'a>(&'a self) -> &'a conduit::Headers {
        &self.headers as &conduit::Headers
    }

    fn body<'a>(&'a mut self) -> &'a mut Reader {
        self as &mut Reader
    }
}

pub fn response<S: ToStatusCode, R: Reader + Send>(status: S,
    headers: HashMap<String, Vec<String>>, body: R) -> conduit::Response
{
    conduit::Response {
        status: status.to_status().unwrap().to_code(),
        headers: headers,
        body: box body as Box<Reader + Send>
    }
}

impl<'a> Connection<'a> {
    fn new<'a>(conn: &'a raw::Connection) -> Result<Connection<'a>, String> {
        match request_info(conn) {
            Ok(info) => {
                let request = CivetRequest { conn: conn, request_info: info, headers: Headers { conn: conn } };
                Ok(Connection {
                    request: request,
                    written: false,
                })
            },
            Err(err) => Err(err)
        }
    }

}

impl<'a> Writer for Connection<'a> {
    fn write(&mut self, buf: &[u8]) -> IoResult<()> {
        self.written = true;
        write_bytes(self.request.conn, buf).map_err(|_| {
            io::standard_error(io::IoUnavailable)
        })
    }
}

impl<'a> Reader for CivetRequest<'a> {
    fn read(&mut self, buf: &mut[u8]) -> IoResult<uint> {
        let ret = raw::read(self.conn, buf);

        if ret == 0 {
            Err(io::standard_error(io::EndOfFile))
        } else {
            Ok(ret as uint)
        }
    }
}

#[unsafe_destructor]
impl<'a> Drop for Connection<'a> {
    fn drop(&mut self) {
        if !self.written {
            let _ = writeln!(self, "HTTP/1.1 500 Internal Server Error");
        }
    }
}

pub struct Headers<'a> {
    conn: &'a raw::Connection
}

impl<'a> conduit::Headers for Headers<'a> {
    fn find<'a>(&'a self, string: &str) -> Option<Vec<&'a str>> {
        get_header(self.conn, string).map(|s| vec!(s))
    }

    fn has(&self, string: &str) -> bool {
        get_header(self.conn, string).is_some()
    }

    fn iter<'a>(&'a self) -> conduit::HeaderEntries<'a> {
        box HeaderIterator::new(self.conn) as conduit::HeaderEntries<'a>
    }
}

pub struct HeaderIterator<'a> {
    headers: Vec<Header<'a>>,
    position: uint
}

impl<'a> HeaderIterator<'a> {
    fn new<'b>(conn: &'b raw::Connection) -> HeaderIterator<'b> {
        HeaderIterator { headers: get_headers(conn), position: 0 }
    }
}

impl<'a> Iterator<(&'a str, Vec<&'a str>)> for HeaderIterator<'a> {
    fn next(&mut self) -> Option<(&'a str, Vec<&'a str>)> {
        let pos = self.position;
        let headers = &self.headers;

        if self.headers.len() <= pos {
            None
        } else {
            let header = headers.get(pos);
            self.position += 1;
            header.name().map(|name| (name, vec!(header.value().unwrap())))
        }
    }
}

pub struct Server<E>(raw::Server<Box<Handler<E> + 'static + Share>>);

impl<E> Server<E> {
    pub fn start<H: Handler<E> + 'static + Share>(options: Config, handler: H)
        -> IoResult<Server<E>>
    {
        fn internal_handler<E>(conn: &mut raw::Connection,
                            handler: &Box<Handler<E>>) -> Result<(), ()> {
            let mut connection = Connection::new(conn).unwrap();
            let response = handler.call(&mut connection.request);
            let writer = &mut connection;

            fn err<W: Writer>(writer: &mut W) {
                let _ = writeln!(writer, "HTTP/1.1 500 Internal Server Error");
            }

            let conduit::Response { status, headers, mut body } = match response {
                Ok(r) => r,
                Err(_) => return Err(err(writer)),
            };
            let (code, string) = status;
            try!(write!(writer, "HTTP/1.1 {} {}\r\n", code, string).map_err(|_| ()));

            for (key, value) in headers.iter() {
                try!(write!(writer, "{}: {}\r\n", key, value).map_err(|_| ()));
            }

            try!(write!(writer, "\r\n").map_err(|_| ()));
            try!(util::copy(&mut body, writer).map_err(|_| ()));

            Ok(())
        }

        let raw_callback = raw::ServerCallback::new(internal_handler,
                                                    box handler);
        Ok(Server(try!(raw::Server::start(options, raw_callback))))
    }
}

fn write_bytes(connection: &raw::Connection, bytes: &[u8]) -> Result<(), String> {
    let ret = raw::write(connection, bytes);

    if ret == -1 {
        return Err("Couldn't write bytes to the connection".to_str())
    }

    Ok(())
}

fn request_info<'a>(connection: &'a raw::Connection)
    -> Result<RequestInfo<'a>, String>
{
    match get_request_info(connection) {
        Some(info) => Ok(info),
        None => Err("Couldn't get request info for connection".to_str())
    }
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;
    use std::io::net::ip::SocketAddr;
    use std::io::net::tcp::TcpStream;
    use std::io::test::next_test_ip4;
    use std::io::{IoResult, MemReader};
    use std::sync::Mutex;
    use super::{Server, Config, Request, Response, Handler};

    fn noop(_: &mut Request) -> IoResult<Response> { unreachable!() }

    fn request(addr: SocketAddr, req: &str) -> String {
        let mut s = TcpStream::connect(addr.ip.to_str().as_slice(),
                                       addr.port).unwrap();
        s.write_str(req.trim_left()).unwrap();
        s.read_to_str().unwrap()
    }

    #[test]
    fn smoke() {
        let addr = next_test_ip4();
        Server::start(Config { port: addr.port, threads: 1 }, noop).unwrap();
    }

    #[test]
    fn dupe_port() {
        let addr = next_test_ip4();
        let s1 = Server::start(Config { port: addr.port, threads: 1 }, noop);
        assert!(s1.is_ok());
        let s2 = Server::start(Config { port: addr.port, threads: 1 }, noop);
        assert!(s2.is_err());
    }

    #[test]
    fn drops_handler() {
        static mut DROPPED: bool = false;
        struct Foo;
        impl Handler for Foo {
            fn call(&self, _req: &mut Request) -> IoResult<Response> {
                fail!()
            }
        }
        impl Drop for Foo {
            fn drop(&mut self) { unsafe { DROPPED = true; } }
        }

        let addr = next_test_ip4();
        drop(Server::start(Config { port: addr.port, threads: 1 }, Foo));
        unsafe { assert!(DROPPED); }
    }

    #[test]
    fn invokes() {
        struct Foo(Mutex<Sender<()>>);
        impl Handler for Foo {
            fn call(&self, _req: &mut Request) -> IoResult<Response> {
                let Foo(ref tx) = *self;
                tx.lock().send(());
                Ok(Response::new(200, HashMap::new(), MemReader::new(vec![])))
            }
        }

        let addr = next_test_ip4();
        let (tx, rx) = channel();
        let handler = Foo(Mutex::new(tx));
        let _s = Server::start(Config { port: addr.port, threads: 1 }, handler);
        request(addr, r"
GET / HTTP/1.1

");
        rx.recv();
    }

    #[test]
    fn header_sent() {
        struct Foo(Mutex<Sender<String>>);
        impl Handler for Foo {
            fn call(&self, req: &mut Request) -> IoResult<Response> {
                let Foo(ref tx) = *self;
                tx.lock().send(req.get_header("Foo").unwrap());
                Ok(Response::new(200, HashMap::new(), MemReader::new(vec![])))
            }
        }

        let addr = next_test_ip4();
        let (tx, rx) = channel();
        let handler = Foo(Mutex::new(tx));
        let _s = Server::start(Config { port: addr.port, threads: 1 }, handler);
        request(addr, r"
GET / HTTP/1.1
Foo: bar

");
        assert_eq!(rx.recv().as_slice(), "bar");
    }

    #[test]
    fn failing_handler() {
        struct Foo;
        impl Handler for Foo {
            fn call(&self, _req: &mut Request) -> IoResult<Response> {
                fail!()
            }
        }

        let addr = next_test_ip4();
        let _s = Server::start(Config { port: addr.port, threads: 1 }, Foo);
        request(addr, r"
GET / HTTP/1.1
Foo: bar

");
    }

    #[test]
    fn failing_handler_is_500() {
        struct Foo;
        impl Handler for Foo {
            fn call(&self, _req: &mut Request) -> IoResult<Response> {
                fail!()
            }
        }

        let addr = next_test_ip4();
        let _s = Server::start(Config { port: addr.port, threads: 1 }, Foo);
        let response = request(addr, r"
GET / HTTP/1.1
Foo: bar

");
        assert!(response.as_slice().contains("500 Internal"),
                "not a failing response: {}", response);
    }
}
