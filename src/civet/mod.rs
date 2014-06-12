use std;
use std::mem::transmute;
use std::ptr::null;
use std::io;
use std::io::IoResult;
use std::c_str::CString;
use libc;
use libc::{c_void,c_char,c_int,c_long,size_t};
use native;

#[link(name="civetweb")]
extern {
    fn mg_start(callbacks: *MgCallbacks, user_data: *c_void, options: **c_char) -> *MgContext;
    fn mg_set_request_handler(context: *MgContext, uri: *c_char, handler: MgRequestHandler, data: *c_void);
    fn mg_write(connection: *MgConnection, data: *c_void, len: size_t) -> c_int;
    fn mg_get_header(connection: *MgConnection, name: *c_char) -> *c_char;
    fn mg_get_request_info(connection: *MgConnection) -> *MgRequestInfo;
}

enum MgContext {}
enum MgConnection {}

type MgRequestHandler = fn(*MgConnection, *c_void) -> int;

#[allow(dead_code)]
struct MgHeader {
    name: *c_char,
    value: *c_char
}

#[allow(dead_code)]
struct MgRequestInfo {
    request_method: *c_char,
    uri: *c_char,
    http_version: *c_char,
    query_string: *c_char,
    remote_user: *c_char,
    remote_ip: c_long,
    remote_port: c_int,
    is_ssl: bool,
    user_data: *c_void,
    conn_data: *c_void,

    num_headers: c_int,
    headers: [MgHeader, ..64]
}

#[allow(dead_code)]
struct MgCallbacks {
    begin_request: *c_void,
    end_request: *c_void,
    log_message: *c_void,
    init_ssl: *c_void,
    websocket_connect: *c_void,
    websocket_ready: *c_void,
    websocket_data: *c_void,
    connection_close: *c_void,
    open_file: *c_void,
    init_lua: *c_void,
    upload: *c_void,
    http_error: *c_void
}

impl MgCallbacks {
    pub fn new() -> MgCallbacks {
        MgCallbacks {
            begin_request: null(),
            end_request: null(),
            log_message: null(),
            init_ssl: null(),
            websocket_connect: null(),
            websocket_ready: null(),
            websocket_data: null(),
            connection_close: null(),
            open_file: null(),
            init_lua: null(),
            upload: null(),
            http_error: null()
        }
    }
}

pub struct Config {
    pub port: uint,
    pub threads: uint
}

impl Config {
    pub fn default() -> Config {
        Config { port: 8888, threads: 50 }
    }
}

pub struct Connection<'a> {
    conn: &'a MgConnection,
    request_info: &'a MgRequestInfo
}

impl<'a> Connection<'a> {
    pub fn new<'a>(conn: &'a MgConnection) -> Result<Connection<'a>, String> {
        match request_info(conn) {
            Ok(info) => Ok(Connection { conn: conn, request_info: info }),
            Err(err) => Err(err)
        }
    }

    pub fn get_header<S: Str>(&self, string: S) -> Option<String> {
        get_header(self.conn, string.as_slice())
    }

    pub fn count_headers(&self) -> Result<uint, String> {
        headers_len(self.conn)
    }

    pub fn method(&self) -> Option<String> {
        self.info_to_str(|i| i.request_method)
    }

    pub fn url(&self) -> Option<String> {
        self.info_to_str(|i| i.uri)
    }

    pub fn http_version(&self) -> Option<String> {
        self.info_to_str(|i| i.http_version)
    }

    pub fn query_string(&self) -> Option<String> {
        self.info_to_str(|i| i.query_string)
    }

    pub fn remote_user(&self) -> Option<String> {
        self.info_to_str(|i| i.remote_user)
    }

    pub fn remote_ip(&self) -> int {
        self.with_info(|i| i.remote_ip as int)
    }

    pub fn is_ssl(&self) -> bool {
        self.with_info(|i| i.is_ssl)
    }

    pub fn headers<'a>(&'a self) -> Headers<'a> {
        Headers { connection: self }
    }

    fn info_to_str(&self, callback: |&MgRequestInfo| -> *c_char) -> Option<String> {
        to_str(callback(self.request_info))
    }

    fn with_info<T>(&self, callback: |&MgRequestInfo| -> T) -> T {
        callback(self.request_info)
    }
}

impl<'a> Writer for Connection<'a> {
    fn write(&mut self, buf: &[u8]) -> IoResult<()> {
        write_bytes(self.conn, buf).map_err(|_| {
            io::standard_error(io::IoUnavailable)
        })
    }
}

pub struct Headers<'a> {
    connection: &'a Connection<'a>
}

impl<'a> Headers<'a> {
    pub fn find<S: Str>(&self, string: S) -> Option<String> {
        self.connection.get_header(string)
    }

    pub fn iter<'a>(&'a self) -> HeaderIterator<'a> {
        HeaderIterator::new(self.connection)
    }
}

pub struct HeaderIterator<'a> {
    connection: &'a Connection<'a>,
    position: uint
}

impl<'a> HeaderIterator<'a> {
    fn new<'a>(connection: &'a Connection) -> HeaderIterator<'a> {
        HeaderIterator { connection: connection, position: 0 }
    }
}

impl<'a> Iterator<(String, String)> for HeaderIterator<'a> {
    fn next(&mut self) -> Option<(String, String)> {
        let pos = self.position;

        match get_headers(self.connection.conn).ok() {
            Some(headers) => {
                let header = headers[pos];

                if header.name.is_null() {
                    return None;
                }

                self.position += 1;

                to_str(header.name).map(|name| {
                    (name, to_str(header.value).unwrap())
                })
            },
            None => None
        }
    }
}

#[allow(dead_code)]
pub struct Server {
    context: *MgContext,
}

impl Server {
    pub fn start(options: Config, handler: fn(Connection) -> IoResult<()>) -> IoResult<Server> {
        let Config { port, threads } = options;
        let options = ["listening_ports".to_str(), port.to_str(), "num_threads".to_str(), threads.to_str()];

        fn internal_handler(conn: *MgConnection, handler: *c_void) -> int {
            let _ = Connection::new(unsafe { conn.to_option() }.unwrap()).map(|connection| {
                let (tx, rx) = channel();
                let handler: fn(Connection) -> IoResult<()> = unsafe { transmute(handler) };
                let mut task = native::task::new((0, std::uint::MAX));

                task.death.on_exit = Some(proc(r) tx.send(r));
                task.run(|| { println!("Made it so far"); let _ = handler(connection); println!("Done"); });
                let _ = rx.recv();
            });

            1
        }

        let mut server = None;

        options.with_c_strs(true, |options: **c_char| {
            let context = unsafe { mg_start(&MgCallbacks::new(), transmute(handler), options) };
            server = Some(Server { context: context });

            unsafe { mg_set_request_handler(context, "**".to_c_str().unwrap(), internal_handler, transmute(handler)) };
        });

        Ok(server.unwrap())
    }
}


fn write_bytes(connection: *MgConnection, bytes: &[u8]) -> Result<(), String> {
    let c_bytes = bytes.as_ptr() as *c_void;
    let ret = unsafe { mg_write(connection, c_bytes, bytes.len() as u64) };

    if ret == -1 {
        return Err("Couldn't write bytes to the connection".to_str())
    }

    Ok(())
}

fn get_header<'a>(connection: &'a MgConnection, string: &str) -> Option<String> {
    let cstr = unsafe { mg_get_header(connection, string.to_c_str().unwrap()).to_option() };

    cstr.map(|c| unsafe { CString::new(c, false) }.as_str().to_str())
}

fn to_str(string: *c_char) -> Option<String> {
    unsafe {
        match string.to_option() {
            None => None,
            Some(c) => {
                if *string == 0 {
                    return None;
                }

                match CString::new(c, false).as_str() {
                    Some(s) => Some(s.to_str()),
                    None => None
                }
            }
        }
    }
}

fn get_headers<'a>(connection: &'a MgConnection) -> Result<[MgHeader, ..64], String> {
    let request_info = unsafe { mg_get_request_info(connection) };

    if request_info.is_null() {
        Err("Couldn't get request info for connection".to_str())
    } else {
        let info = unsafe { *request_info };
        Ok(info.headers)
    }
}

fn headers_len<'a>(connection: &'a MgConnection) -> Result<uint, String> {
    let info = try!(request_info(connection));
    Ok(info.num_headers as uint)
}

fn request_info<'a>(connection: &'a MgConnection) -> Result<&'a MgRequestInfo, String> {
    let request_info = unsafe { mg_get_request_info(connection) };

    if request_info.is_null() {
        Err("Couldn't get request info for connection".to_str())
    } else {
        Ok(unsafe { transmute(request_info) })
    }
}

trait WithCStrs {
    fn with_c_strs(&self, null_terminated: bool, f: |**libc::c_char|) ;
}

impl<'a, T: ToCStr> WithCStrs for &'a [T] {
    fn with_c_strs(&self, null_terminate: bool, f: |**c_char|) {
        let c_strs: Vec<CString> = self.iter().map(|s: &T| s.to_c_str()).collect();
        let mut ptrs: Vec<*c_char> = c_strs.iter().map(|c: &CString| c.with_ref(|ptr| ptr)).collect();
        if null_terminate {
            ptrs.push(null());
        }
        f(ptrs.as_ptr())
    }
}
