use std::thread;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use serde_json;
use regex::Regex;
use {MockResponse, Matcher, SERVER_ADDRESS, Request};

#[derive(Serialize, Deserialize, Debug)]
struct RemoteMock {
    id: String,
    method: String,
    path: Matcher,
    headers: Vec<(String, Matcher)>,
    body: Matcher,
    response: MockResponse,
    hits: usize,
    expected_hits: usize,
}

impl RemoteMock {
    fn method_matches(&self, request: &Request) -> bool {
        self.method == request.method
    }

    fn path_matches(&self, request: &Request) -> bool {
        self.path == request.path
    }

    fn headers_match(&self, request: &Request) -> bool {
        for &(ref field, ref value) in &self.headers {
            match request.find_header(field) {
                Some(request_header_value) => {
                    if value == request_header_value { continue }

                    return false
                },
                None => {
                    if value == &Matcher::Missing { continue }

                    return false
                },
            }
        }

        true
    }

    fn body_matches(&self, request: &Request) -> bool {
        self.body == String::from_utf8_lossy(&request.body).into_owned()
    }
}

impl<'a> PartialEq<Request> for &'a mut RemoteMock {
    fn eq(&self, other: &Request) -> bool {
        self.method_matches(other)
            && self.path_matches(other)
            && self.headers_match(other)
            && self.body_matches(other)
    }
}

struct State {
    mocks: Vec<RemoteMock>,
    unmatched_requests: Vec<Request>,
}

impl Default for State {
    fn default() -> Self {
        State {
            mocks: Vec::new(),
            unmatched_requests: Vec::new(),
        }
    }
}

pub fn try_start() {
    if is_listening() { return }

    start()
}

fn start() {
    thread::spawn(move || {
        let mut state = State::default();
        let listener = TcpListener::bind(SERVER_ADDRESS).unwrap();
        for stream in listener.incoming() {
            match stream {
                Ok(mut stream) => {
                    let request = Request::from(&mut stream);
                    if request.is_ok() {
                        handle_request(&mut state, request, stream);
                    } else {
                        let body = request.error().map_or("Could not parse the request.", |err| err.as_str());
                        let response = format!("HTTP/1.1 422 Unprocessable Entity\r\ncontent-length: {}\r\n\r\n{}", body.len(), body);
                        stream.write(response.as_bytes()).unwrap();
                    }
                },
                Err(_) => {},
            }
        }
    });

    while !is_listening() {}
}

fn is_listening() -> bool {
    TcpStream::connect(SERVER_ADDRESS).is_ok()
}

fn handle_request(mut state: &mut State, request: Request, stream: TcpStream) {
    lazy_static! {
        static ref GET_MOCK_REGEX: Regex = Regex::new(r"^GET /mockito/mocks/(?P<mock_id>\w+)$").unwrap();
        static ref POST_MOCKS_REGEX: Regex = Regex::new(r"^POST /mockito/mocks$").unwrap();
        static ref DELETE_MOCKS_REGEX: Regex = Regex::new(r"^DELETE /mockito/mocks$").unwrap();
        static ref DELETE_MOCK_REGEX: Regex = Regex::new(r"^DELETE /mockito/mocks/(?P<mock_id>\w+)$").unwrap();
        static ref LAST_REQUEST: Regex = Regex::new(r"^GET /mockito/last_unmatched_request$").unwrap();
    }

    let request_line = format!("{} {}", request.method, request.path);

    if let Some(captures) = GET_MOCK_REGEX.captures(&request_line) {
        return handle_get_mock(state, captures["mock_id"].to_string(), stream);
    }

    if let Some(_) = POST_MOCKS_REGEX.captures(&request_line) {
        return handle_post_mock(state, request, stream);
    }

    if let Some(_) = DELETE_MOCKS_REGEX.captures(&request_line) {
        return handle_delete_mocks(state, stream);
    }

    if let Some(captures) = DELETE_MOCK_REGEX.captures(&request_line) {
        return handle_delete_mock(state, captures["mock_id"].to_string(), stream);
    }

    if let Some(_) = LAST_REQUEST.captures(&request_line) {
        return handle_last_unmatched_request(state, stream);
    }

    handle_match_mock(state, request, stream);
}

fn handle_get_mock(state: &mut State, mock_id: String, mut stream: TcpStream) {
    match state.mocks.iter().find(|mock| mock.id == mock_id) {
        Some(mock) => {
            let body = serde_json::to_string(mock).unwrap();
            let response = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{}", body.len(), body);
            stream.write(response.as_bytes()).unwrap();
        },
        None => {
            stream.write("HTTP/1.1 404 Not Found\r\n\r\n".as_bytes()).unwrap();
        },
    }
}

fn handle_post_mock(mut state: &mut State, request: Request, mut stream: TcpStream) {
    match serde_json::from_slice::<RemoteMock>(&request.body) {
        Ok(mock) => {
            state.mocks.push(mock);
            stream.write("HTTP/1.1 200 OK\r\n\r\n".as_bytes()).unwrap();
        },
        Err(err) => {
            let message = err.to_string();
            let response = format!("HTTP/1.1 422 Unprocessable Entity\r\ncontent-length: {}\r\n\r\n{}", message.len(), message);
            stream.write(response.as_bytes()).unwrap();
        }
    }
}

fn handle_delete_mocks(mut state: &mut State, mut stream: TcpStream) {
    state.mocks.clear();
    stream.write("HTTP/1.1 200 OK\r\n\r\n".as_bytes()).unwrap();
}

fn handle_delete_mock(mut state: &mut State, mock_id: String, mut stream: TcpStream) {
    match state.mocks.iter().position(|mock| mock.id == mock_id) {
        Some(pos) => {
            state.mocks.remove(pos);
            stream.write("HTTP/1.1 200 OK\r\n\r\n".as_bytes()).unwrap();
        },
        None => {
            stream.write("HTTP/1.1 404 Not Found\r\n\r\n".as_bytes()).unwrap();
        },
    };
}

fn handle_last_unmatched_request(state: &mut State, mut stream: TcpStream) {
    let body = match state.unmatched_requests.last() {
        Some(request) => format!("{}", request),
        None => String::new(),
    };

    let response = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{}", body.len(), body);
    stream.write(response.as_bytes()).unwrap();
}

fn handle_match_mock(state: &mut State, request: Request, mut stream: TcpStream) {
    match state.mocks.iter_mut().rev().find(|mock| mock == &request) {
        Some(mut mock) => {
            mock.hits = mock.hits + 1;

            let mut headers = String::new();
            for &(ref key, ref value) in &mock.response.headers {
                headers.push_str(key);
                headers.push_str(": ");
                headers.push_str(value);
                headers.push_str("\r\n");
            }

            let ref body = mock.response.body;

            let response = format!("HTTP/1.1 {}\r\ncontent-length: {}\r\n{}\r\n{}", mock.response.status, body.len(), headers, body);
            stream.write(response.as_bytes()).unwrap();
        },
        None => {
            state.unmatched_requests.push(request);
            stream.write("HTTP/1.1 501 Not Implemented\r\n\r\n".as_bytes()).unwrap();
        }
    }
}
