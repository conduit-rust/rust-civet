extern crate conduit;
extern crate civet;
extern crate route_recognizer;

use std::collections::HashMap;
use std::error::Error;
use std::io::{self, Cursor};
use std::sync::mpsc::channel;

use civet::{Config, Server, response};
use conduit::{Request, Response};
use route_recognizer::{Router, Params};

struct MyServer {
    router: Router<fn(&mut dyn Request, &Params) -> io::Result<Response>>,
}

impl conduit::Handler for MyServer {
    fn call(&self, req: &mut dyn Request) -> Result<Response, Box<dyn Error+Send>> {
        let hit = match self.router.recognize(req.path()) {
            Ok(m) => m,
            Err(e) => panic!("{}", e),
        };
        (*hit.handler)(req, &hit.params).map_err(|e| Box::new(e) as Box<dyn Error+Send>)
    }
}

fn main() {
    let mut server = MyServer {
        router: Router::new(),
    };
    server.router.add("/:id", id);
    server.router.add("/", root);
    let _a = Server::start(Config::new(), server);
    let (_tx, rx) = channel::<()>();
    rx.recv().unwrap();
}

fn root(_req: &mut dyn Request, _params: &Params) -> io::Result<Response> {
    let bytes = b"you found the root!\n".to_vec();
    Ok(response(200, HashMap::new(), Cursor::new(bytes)))
}

fn id(_req: &mut dyn Request, params: &Params) -> io::Result<Response> {
    let string = format!("you found the id {}!\n", params["id"]);
    let bytes = string.into_bytes();

    Ok(response(200, HashMap::new(), Cursor::new(bytes)))
}
