use log::*;
use std::collections::{HashMap, VecDeque};
use std::fmt::Write;

const MAX_COMMAND_LEN: usize = 3;
const MAX_KEY_LEN: usize = 250;
const MAX_FLAGS_DIGITS_LEN: usize = 10;
const MAX_SIZE_DIGITS_LEN: usize = 20;

const ERROR_RESPONSE: &'static [u8] = b"ERROR\r\n";

#[derive(Debug)]
enum State {
    ReadingCommand(heapless::Vec<u8, MAX_COMMAND_LEN>),
    ReadingKey {
        cmd: CommandWithKey,
        key: heapless::Vec<u8, MAX_KEY_LEN>,
    },
    SendingError {
        flush_line: bool,
        remaining: &'static [u8],
        #[allow(dead_code)]
        error: Error,
    },
    FlushLine,
    SendingGetVALUE {
        remaining: &'static [u8],
        key: heapless::Vec<u8, MAX_KEY_LEN>,
        entry: *const Entry,
    },
    SendingGetKey {
        key: heapless::Vec<u8, MAX_KEY_LEN>,
        sent: usize,
        entry: *const Entry,
    },
    SendingGetKeySpace {
        entry: *const Entry,
    },
    SendingGetFlags {
        data: heapless::Vec<u8, MAX_FLAGS_DIGITS_LEN>,
        sent: usize,
        entry: *const Entry,
    },
    SendingGetFlagsSpace {
        entry: *const Entry,
    },
    SendingGetLen {
        data: heapless::Vec<u8, MAX_SIZE_DIGITS_LEN>,
        sent: usize,
        entry: *const Entry,
    },
    SendingGetNewline {
        entry: *const Entry,
    },
    SendingGetData {
        entry: *const Entry,
        sent: usize,
    },
    SendingEnd {
        remaining: &'static [u8],
    },
}

impl State {
    fn wants_to_send(&self) -> bool {
        match self {
            Self::SendingError { .. } => true,
            Self::SendingGetVALUE { .. } => true,
            Self::SendingGetKey { .. } => true,
            Self::SendingGetFlags { .. } => true,
            Self::SendingGetFlagsSpace { .. } => true,
            Self::SendingGetLen { .. } => true,
            Self::SendingGetNewline { .. } => true,
            Self::SendingGetData { .. } => true,
            Self::SendingEnd { .. } => true,
            _ => false,
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self::ReadingCommand(Default::default())
    }
}

#[derive(Debug)]
enum Error {
    UnknownCommand,
    CommandTooLong,
    KeyTooLong,
    MissingArgument,
}

#[derive(Debug)]
enum CommandWithKey {
    Get,
    Set,
}

pub struct CommandHandler {
    state: State,
    data: HashMap<Vec<u8>, Entry>,
}

impl CommandHandler {
    pub fn new(data: HashMap<Vec<u8>, Entry>) -> Self {
        Self {
            state: Default::default(),
            data,
        }
    }
}

pub struct Entry {
    flags: u32,
    value: Vec<u8>,
}

impl Entry {
    pub fn new(value: Vec<u8>) -> Self {
        Self { flags: 0, value }
    }
}

pub trait Socket {
    fn receive<R>(&mut self, f: impl FnOnce(&[u8]) -> R) -> Option<R>;
    fn transmit<R>(&mut self, f: impl FnOnce(&mut [u8]) -> (usize, R)) -> Option<R>;
}

impl CommandHandler {
    pub fn poll(&mut self, s: &mut impl Socket) -> bool {
        // Send if we need to

        let mut write_happened = false;

        if self.state.wants_to_send() {
            write_happened = s
                .transmit(|mut buf| {
                    let mut bytes_produced = 0;
                    while buf.len() > 0 {
                        info!("{:?}", self.state);
                        match &mut self.state {
                            State::SendingError {
                                remaining,
                                flush_line,
                                ..
                            } => {
                                let n = std::cmp::min(buf.len(), remaining.len());
                                if n > 0 {
                                    buf[..n].copy_from_slice(&remaining[..n]);
                                    buf = &mut buf[n..];
                                    bytes_produced += n;
                                    *remaining = &remaining[n..];
                                    if remaining.len() == 0 {
                                        self.state = if *flush_line {
                                            State::FlushLine
                                        } else {
                                            Default::default()
                                        };
                                    }
                                }
                            }
                            State::SendingGetVALUE {
                                remaining,
                                key,
                                entry,
                            } => {
                                let n = std::cmp::min(buf.len(), remaining.len());
                                if n > 0 {
                                    buf[..n].copy_from_slice(&remaining[..n]);
                                    *remaining = &remaining[n..];
                                    buf = &mut buf[n..];
                                    bytes_produced += n;
                                    if remaining.len() == 0 {
                                        self.state = State::SendingGetKey {
                                            key: key.clone(),
                                            sent: 0,
                                            entry: *entry,
                                        };
                                    }
                                }
                            }
                            State::SendingGetKey { key, sent, entry } => {
                                let remaining = &key.as_slice()[*sent..];
                                let n = std::cmp::min(buf.len(), remaining.len());
                                if n > 0 {
                                    buf[..n].copy_from_slice(&remaining[..n]);
                                    buf = &mut buf[n..];
                                    bytes_produced += n;
                                    *sent += n;
                                    if *sent == key.len() {
                                        self.state = State::SendingGetKeySpace { entry: *entry };
                                    }
                                }
                            }
                            State::SendingGetKeySpace { entry } => {
                                buf[0] = b' ';
                                buf = &mut buf[1..];
                                bytes_produced += 1;
                                let mut flags_str =
                                    heapless::Vec::<u8, MAX_FLAGS_DIGITS_LEN>::new();
                                // SAFETY: We promise we don't modify the map during GET flow
                                let e = unsafe { &**entry };
                                write!(flags_str, "{}", e.flags).expect("formatting flags");
                                self.state = State::SendingGetFlags {
                                    entry: *entry,
                                    data: flags_str,
                                    sent: 0,
                                };
                            }
                            State::SendingGetFlags { data, sent, entry } => {
                                let remaining = &data.as_slice()[*sent..];
                                let n = std::cmp::min(buf.len(), remaining.len());
                                if n > 0 {
                                    buf[..n].copy_from_slice(&remaining[..n]);
                                    buf = &mut buf[n..];
                                    bytes_produced += n;
                                    *sent += n;
                                    if *sent == data.len() {
                                        self.state = State::SendingGetFlagsSpace { entry: *entry };
                                    }
                                }
                            }
                            State::SendingGetFlagsSpace { entry } => {
                                buf[0] = b' ';
                                buf = &mut buf[1..];
                                bytes_produced += 1;

                                let mut len_str = heapless::Vec::<u8, MAX_SIZE_DIGITS_LEN>::new();
                                // SAFETY: We promise we don't modify the map during GET flow
                                let e = unsafe { &**entry };
                                write!(len_str, "{}", e.value.len()).expect("formatting len");
                                self.state = State::SendingGetLen {
                                    entry: *entry,
                                    data: len_str,
                                    sent: 0,
                                };
                            }
                            State::SendingGetLen { data, sent, entry } => {
                                let remaining = &data.as_slice()[*sent..];
                                let n = std::cmp::min(buf.len(), remaining.len());
                                if n > 0 {
                                    buf[..n].copy_from_slice(&remaining[..n]);
                                    buf = &mut buf[n..];
                                    bytes_produced += n;
                                    *sent += n;
                                    if *sent == data.len() {
                                        self.state = State::SendingGetNewline { entry: *entry };
                                    }
                                }
                            }
                            State::SendingGetNewline { entry } => {
                                buf[0] = b'\n';
                                buf = &mut buf[1..];
                                bytes_produced += 1;
                                self.state = State::SendingGetData {
                                    entry: *entry,
                                    sent: 0,
                                };
                            }
                            State::SendingGetData { sent, entry } => {
                                // SAFETY: We promise we don't modify the map during GET flow
                                let e = unsafe { &**entry };
                                let remaining = &e.value[*sent..];
                                let n = std::cmp::min(buf.len(), remaining.len());
                                if n > 0 {
                                    buf[..n].copy_from_slice(&remaining[..n]);
                                    buf = &mut buf[n..];
                                    bytes_produced += n;
                                    *sent += n;
                                    if *sent == e.value.len() {
                                        self.state = State::SendingEnd {
                                            remaining: b"\r\nEND\r\n",
                                        };
                                    }
                                }
                            }
                            State::SendingEnd { remaining } => {
                                let n = std::cmp::min(buf.len(), remaining.len());
                                if n > 0 {
                                    buf[..n].copy_from_slice(&remaining[..n]);
                                    *remaining = &remaining[n..];
                                    buf = &mut buf[n..];
                                    bytes_produced += n;
                                    if remaining.len() == 0 {
                                        self.state = Default::default();
                                    }
                                }
                            }
                            _ => break,
                        }
                    }
                    (bytes_produced, ())
                })
                .is_some();
        }

        let recv_happened = s
            .receive(|data| {
                for c in data.iter().copied() {
                    info!("{:?} {:?}", self.state, c as char);
                    match (&mut self.state, c) {
                        (State::ReadingCommand(cmd), b' ' | b'\n') => {
                            let cmd = match cmd.as_slice() {
                                b"get" => CommandWithKey::Get,
                                b"set" => CommandWithKey::Set,
                                _ => {
                                    self.state = State::SendingError {
                                        flush_line: c == b' ',
                                        remaining: ERROR_RESPONSE,
                                        error: Error::UnknownCommand,
                                    };
                                    continue;
                                }
                            };
                            if c == b'\n' {
                                self.state = State::SendingError {
                                    flush_line: false,
                                    remaining: ERROR_RESPONSE,
                                    error: Error::MissingArgument,
                                };
                                continue;
                            }
                            self.state = State::ReadingKey {
                                cmd,
                                key: Default::default(),
                            };
                        }
                        (State::ReadingCommand(cmd), _) => {
                            if !cmd.push(c).is_ok() {
                                self.state = State::SendingError {
                                    flush_line: true,
                                    remaining: ERROR_RESPONSE,
                                    error: Error::CommandTooLong,
                                };
                                continue;
                            }
                        }
                        (State::ReadingKey { cmd, key }, b' ' | b'\n') => {
                            // We read a key, process it with the command
                            match cmd {
                                CommandWithKey::Get => {
                                    if let Some(entry) = self.data.get(key.as_slice()) {
                                        self.state = State::SendingGetVALUE {
                                            remaining: b"VALUE ",
                                            key: key.clone(),
                                            entry: entry as *const _,
                                        };
                                    } else {
                                        if c == b'\n' {
                                            self.state = State::SendingEnd {
                                                remaining: b"END\r\n",
                                            };
                                        }
                                    }
                                }
                                CommandWithKey::Set => todo!(),
                            }
                        }
                        (State::ReadingKey { key, .. }, _) => {
                            if !key.push(c).is_ok() {
                                self.state = State::SendingError {
                                    flush_line: true,
                                    remaining: ERROR_RESPONSE,
                                    error: Error::KeyTooLong,
                                };
                                continue;
                            }
                        }
                        (State::SendingError { flush_line, .. }, c) => {
                            if *flush_line {
                                if c == b'\n' {
                                    *flush_line = false;
                                }
                            }
                        }
                        (State::FlushLine, c) => {
                            if c == b'\n' {
                                self.state = Default::default();
                            }
                        }
                        (State::SendingGetVALUE { .. }, _) => {
                            error!("Skipping received data in Sending state");
                        }
                        (State::SendingGetKey { .. }, _) => {
                            error!("Skipping received data in Sending state");
                        }
                        (State::SendingGetKeySpace { .. }, _) => {
                            error!("Skipping received data in Sending state");
                        }
                        (State::SendingGetFlags { .. }, _) => {
                            error!("Skipping received data in Sending state");
                        }
                        (State::SendingGetFlagsSpace { .. }, _) => {
                            error!("Skipping received data in Sending state");
                        }
                        (State::SendingGetLen { .. }, _) => {
                            error!("Skipping received data in Sending state");
                        }
                        (State::SendingGetNewline { .. }, _) => {
                            error!("Skipping received data in Sending state");
                        }
                        (State::SendingGetData { .. }, _) => {
                            error!("Skipping received data in Sending state");
                        }
                        (State::SendingEnd { .. }, _) => {
                            error!("Skipping received data in Sending state");
                        }
                    }
                }
            })
            .is_some();

        write_happened || recv_happened
    }
}

struct MockSocket {
    rbuf: VecDeque<u8>,
}

impl MockSocket {
    pub fn new() -> Self {
        Self {
            rbuf: Default::default(),
        }
    }
}

impl Socket for MockSocket {
    fn receive<R>(&mut self, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
        if !self.rbuf.is_empty() {
            let data = self.rbuf.as_slices().0;
            let r = f(data);
            for _ in 0..data.len() {
                self.rbuf.pop_front();
            }
            Some(r)
        } else {
            None
        }
    }

    fn transmit<R>(&mut self, f: impl FnOnce(&mut [u8]) -> (usize, R)) -> Option<R> {
        let mut buf = [0; 100];
        let (sent, r) = f(&mut buf);
        println!("{}", std::str::from_utf8(&buf[..sent]).unwrap());
        Some(r)
    }
}

fn main() {
    env_logger::init();

    let mut map = HashMap::new();
    map.insert(b"foo".to_vec(), Entry::new(b"bar".to_vec()));
    let mut handler = CommandHandler::new(map);

    let mut s = MockSocket::new();

    s.rbuf.extend(b"get foo\n");
    while handler.poll(&mut s) {}

    s.rbuf.extend(b"ge");
    while handler.poll(&mut s) {}
    s.rbuf.extend(b"t no");
    while handler.poll(&mut s) {}
    s.rbuf.extend(b"ooo");
    while handler.poll(&mut s) {}
    s.rbuf.extend(b"pe\n");
    while handler.poll(&mut s) {}

    s.rbuf.extend(b"toolongcommand\n");
    while handler.poll(&mut s) {}
}
