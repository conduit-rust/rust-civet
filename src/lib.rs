#![crate_id="civet"]
#![crate_type="rlib"]

extern crate libc;
extern crate debug;
extern crate native;
extern crate collections;

use std::io;
use std::io::net::ip::{IpAddr, Ipv4Addr};
use std::io::{IoResult,util};
use std::collections::HashMap;

use raw::{RequestInfo,Header};
use raw::{get_header,get_headers,get_request_info};
use status::{ToStatusCode};

pub use raw::Config;

mod raw;
pub mod status;

pub struct Connection<'a> {
    request: Request<'a>
}

pub struct Request<'a> {
    conn: &'a raw::Connection,
    request_info: RequestInfo<'a>
}

pub struct CopiedRequest {
    pub headers: HashMap<String, String>,
    pub method: Option<String>,
    pub url: Option<String>,
    pub http_version: Option<String>,
    pub query_string: Option<String>,
    pub remote_user: Option<String>,
    pub remote_ip: IpAddr,
    pub remote_port: u16,
    pub is_ssl: bool
}

impl<'a> Request<'a> {
    pub fn copy(&self) -> CopiedRequest {
        let mut headers = HashMap::new();

        for (key, value) in self.headers().iter() {
            headers.insert(key.to_str(), value.to_str());
        }

        CopiedRequest {
            headers: headers,
            method: self.method().map(|a| a.to_str()),
            url: self.url().map(|a| a.to_str()),
            http_version: self.http_version().map(|a| a.to_str()),
            query_string: self.query_string().map(|a| a.to_str()),
            remote_user: self.remote_user().map(|a| a.to_str()),
            remote_ip: self.remote_ip(),
            remote_port: self.remote_port(),
            is_ssl: self.is_ssl()
        }
    }

    pub fn get_header<S: Str>(&mut self, string: S) -> Option<String> {
        get_header(self.conn, string.as_slice())
    }

    pub fn count_headers(&self) -> uint {
        self.request_info.num_headers() as uint
    }

    pub fn method<'a>(&'a self) -> Option<&'a str> {
        self.request_info.method()
    }

    pub fn url<'a>(&'a self) -> Option<&'a str> {
        self.request_info.url()
    }

    pub fn http_version<'a>(&'a self) -> Option<&'a str> {
        self.request_info.http_version()
    }

    pub fn query_string<'a>(&'a self) -> Option<&'a str> {
        self.request_info.query_string()
    }

    pub fn remote_user<'a>(&'a self) -> Option<&'a str> {
        self.request_info.remote_user()
    }

    pub fn remote_ip(&self) -> IpAddr {
        let ip = self.request_info.remote_ip();
        Ipv4Addr((ip >> 24) as u8,
                 (ip >> 16) as u8,
                 (ip >>  8) as u8,
                 (ip >>  0) as u8)
    }

    pub fn remote_port(&self) -> u16 {
        self.request_info.remote_port() as u16
    }

    pub fn is_ssl(&self) -> bool {
        self.request_info.is_ssl()
    }

    pub fn headers<'a>(&'a self) -> Headers<'a> {
        Headers { conn: self.conn }
    }
}

pub struct Response {
    status: status::StatusCode,
    headers: HashMap<String, String>,
    body: Box<Reader + Send>,
}

impl Response {
    pub fn new<S: ToStatusCode, R: Reader + Send>(
        status: S,
        headers: HashMap<String, String>,
        body: R) -> Response
    {
        Response {
            status: status.to_status().unwrap(),
            headers: headers,
            body: box body,
        }
    }
}

impl<'a> Connection<'a> {
    fn new<'a>(conn: &'a raw::Connection) -> Result<Connection<'a>, String> {
        match request_info(conn) {
            Ok(info) => {
                let request = Request { conn: conn, request_info: info };
                Ok(Connection {
                    request: request
                })
            },
            Err(err) => Err(err)
        }
    }

}

impl<'a> Writer for Connection<'a> {
    fn write(&mut self, buf: &[u8]) -> IoResult<()> {
        write_bytes(self.request.conn, buf).map_err(|_| {
            io::standard_error(io::IoUnavailable)
        })
    }
}

impl<'a> Reader for Request<'a> {
    fn read(&mut self, buf: &mut[u8]) -> IoResult<uint> {
        let ret = raw::read(self.conn, buf);

        if ret == 0 {
            Err(io::standard_error(io::EndOfFile))
        } else {
            Ok(ret as uint)
        }
    }
}

pub struct Headers<'a> {
    conn: &'a raw::Connection
}

impl<'a> Headers<'a> {
    pub fn find<S: Str>(&self, string: S) -> Option<String> {
        get_header(self.conn, string.as_slice())
    }

    pub fn iter<'a>(&'a self) -> HeaderIterator<'a> {
        HeaderIterator::new(self.conn)
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

impl<'a> Iterator<(&'a str, &'a str)> for HeaderIterator<'a> {
    fn next(&mut self) -> Option<(&'a str, &'a str)> {
        let pos = self.position;
        let headers = &self.headers;

        if self.headers.len() <= pos {
            None
        } else {
            let header = headers.get(pos);
            self.position += 1;
            header.name().map(|name| (name, header.value().unwrap()))
        }
    }
}

pub trait Handler {
    fn call(&self, req: &mut Request) -> IoResult<Response>;
}

impl Handler for fn(&mut Request) -> IoResult<Response> {
    fn call(&self, req: &mut Request) -> IoResult<Response> {
        (*self)(req)
    }
}

// TODO: Why is 'static needed here and below?
pub struct Server(raw::Server<Box<Handler + 'static + Share>>);

impl Server {
    pub fn start<H: 'static + Handler + Share>(options: Config, handler: H)
        -> IoResult<Server>
    {
        fn internal_handler(conn: &mut raw::Connection,
                            handler: &Box<Handler>) -> Result<(), ()> {
            let mut connection = Connection::new(conn).unwrap();
            let response = handler.call(&mut connection.request);
            let writer = &mut connection;

            fn err<W: Writer>(writer: &mut W) {
                let _ = writeln!(writer, "HTTP/1.1 500 Internal Server Error");
            }

            let Response { status, headers, mut body } = match response {
                Ok(r) => r,
                Err(_) => return Err(err(writer)),
            };
            let (code, string) = status.to_code();
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
}
