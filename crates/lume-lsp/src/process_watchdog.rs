use std::{collections::HashMap, ffi::OsString, fmt, path::PathBuf};

const PREFIX: &str = "LUME_INTERNAL_WATCHDOG_";
const MODE: &str = "LUME_INTERNAL_WATCHDOG_MODE";
const PROGRAM: &str = "LUME_INTERNAL_WATCHDOG_PROGRAM";
const ARG_COUNT: &str = "LUME_INTERNAL_WATCHDOG_ARG_COUNT";
const ARG_PREFIX: &str = "LUME_INTERNAL_WATCHDOG_ARG_";
const CWD_PRESENT: &str = "LUME_INTERNAL_WATCHDOG_CWD_PRESENT";
const CWD: &str = "LUME_INTERNAL_WATCHDOG_CWD";
const ENV_COUNT: &str = "LUME_INTERNAL_WATCHDOG_ENV_COUNT";
const ENV_KEY_PREFIX: &str = "LUME_INTERNAL_WATCHDOG_ENV_KEY_";
const ENV_VALUE_PREFIX: &str = "LUME_INTERNAL_WATCHDOG_ENV_VALUE_";
const PARENT_PID: &str = "LUME_INTERNAL_WATCHDOG_PARENT_PID";
const READINESS_SOCKET: &str = "LUME_INTERNAL_WATCHDOG_READINESS_SOCKET";
const MAX_ITEMS: usize = 4_096;
const READY_MAGIC: &[u8; 8] = b"LUMEWD1\0";
const MAX_READY_ERROR_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchdogCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(OsString, OsString)>,
    pub parent_pid: u32,
    pub readiness_socket: PathBuf,
}

impl WatchdogCommand {
    pub fn to_environment(&self) -> Result<Vec<(OsString, OsString)>, WatchdogProtocolError> {
        if self.args.len() > MAX_ITEMS {
            return Err(WatchdogProtocolError::new("argument count exceeds limit"));
        }
        if self.env.len() > MAX_ITEMS {
            return Err(WatchdogProtocolError::new(
                "environment count exceeds limit",
            ));
        }

        let mut encoded = Vec::with_capacity(8 + self.args.len() + self.env.len() * 2);
        encoded.push(pair(MODE, "1"));
        encoded.push((OsString::from(PROGRAM), self.program.clone()));
        encoded.push(pair(ARG_COUNT, self.args.len().to_string()));
        for (index, argument) in self.args.iter().enumerate() {
            encoded.push((
                OsString::from(format!("{ARG_PREFIX}{index}")),
                argument.clone(),
            ));
        }
        encoded.push(pair(
            CWD_PRESENT,
            if self.cwd.is_some() { "1" } else { "0" },
        ));
        if let Some(cwd) = self.cwd.as_ref() {
            encoded.push((OsString::from(CWD), cwd.as_os_str().to_owned()));
        }
        encoded.push(pair(ENV_COUNT, self.env.len().to_string()));
        for (index, (key, value)) in self.env.iter().enumerate() {
            encoded.push((
                OsString::from(format!("{ENV_KEY_PREFIX}{index}")),
                key.clone(),
            ));
            encoded.push((
                OsString::from(format!("{ENV_VALUE_PREFIX}{index}")),
                value.clone(),
            ));
        }
        encoded.push(pair(PARENT_PID, self.parent_pid.to_string()));
        encoded.push((
            OsString::from(READINESS_SOCKET),
            self.readiness_socket.as_os_str().to_owned(),
        ));
        Ok(encoded)
    }

    pub fn from_environment<I>(environment: I) -> Result<Self, WatchdogProtocolError>
    where
        I: IntoIterator<Item = (OsString, OsString)>,
    {
        let values: HashMap<OsString, OsString> = environment.into_iter().collect();
        if required(&values, MODE)? != "1" {
            return Err(WatchdogProtocolError::new("invalid watchdog mode"));
        }
        let argument_count = parse_count(&values, ARG_COUNT, "argument count")?;
        let environment_count = parse_count(&values, ENV_COUNT, "environment count")?;
        let mut args = Vec::with_capacity(argument_count);
        for index in 0..argument_count {
            args.push(required(&values, &format!("{ARG_PREFIX}{index}"))?.to_owned());
        }
        let cwd = match required(&values, CWD_PRESENT)?.to_str() {
            Some("0") => None,
            Some("1") => Some(PathBuf::from(required(&values, CWD)?)),
            _ => return Err(WatchdogProtocolError::new("invalid cwd presence marker")),
        };
        let mut env = Vec::with_capacity(environment_count);
        for index in 0..environment_count {
            env.push((
                required(&values, &format!("{ENV_KEY_PREFIX}{index}"))?.to_owned(),
                required(&values, &format!("{ENV_VALUE_PREFIX}{index}"))?.to_owned(),
            ));
        }
        let parent_pid = required(&values, PARENT_PID)?
            .to_str()
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|pid| *pid > 1)
            .ok_or_else(|| WatchdogProtocolError::new("invalid parent process ID"))?;

        Ok(Self {
            program: required(&values, PROGRAM)?.to_owned(),
            args,
            cwd,
            env,
            parent_pid,
            readiness_socket: PathBuf::from(required(&values, READINESS_SOCKET)?),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    Ready { server_pid: u32 },
    Error(String),
}

impl Readiness {
    #[allow(clippy::indexing_slicing)] // end è un char boundary <= len
    pub fn encode(&self) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(READY_MAGIC.len() + 5);
        encoded.extend_from_slice(READY_MAGIC);
        match self {
            Self::Ready { server_pid } => {
                encoded.push(1);
                encoded.extend_from_slice(&server_pid.to_be_bytes());
            }
            Self::Error(message) => {
                let mut end = message.len().min(MAX_READY_ERROR_BYTES);
                while !message.is_char_boundary(end) {
                    end -= 1;
                }
                let bytes = &message.as_bytes()[..end];
                encoded.push(2);
                encoded.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
                encoded.extend_from_slice(bytes);
            }
        }
        encoded
    }

    #[allow(clippy::expect_used, clippy::indexing_slicing)] // lunghezze verificate riga per riga prima di ogni slice; payload.len() >= 4 verificato dal braccio; payload.len() == 4 + length verificato
    pub fn decode(bytes: &[u8]) -> Result<Self, WatchdogProtocolError> {
        if bytes.len() < READY_MAGIC.len() + 1 || &bytes[..READY_MAGIC.len()] != READY_MAGIC {
            return Err(WatchdogProtocolError::new("invalid readiness magic"));
        }
        let payload = &bytes[READY_MAGIC.len() + 1..];
        match bytes[READY_MAGIC.len()] {
            1 if payload.len() == 4 => Ok(Self::Ready {
                server_pid: u32::from_be_bytes(payload.try_into().expect("validated PID length")),
            }),
            2 if payload.len() >= 4 => {
                let length =
                    u32::from_be_bytes(payload[..4].try_into().expect("length prefix")) as usize;
                if length > MAX_READY_ERROR_BYTES || payload.len() != 4 + length {
                    return Err(WatchdogProtocolError::new("invalid readiness error length"));
                }
                let message = std::str::from_utf8(&payload[4..])
                    .map_err(|_| WatchdogProtocolError::new("readiness error is not UTF-8"))?;
                Ok(Self::Error(message.to_owned()))
            }
            _ => Err(WatchdogProtocolError::new(
                "invalid readiness message shape",
            )),
        }
    }
}

pub fn is_internal_environment_key(key: &std::ffi::OsStr) -> bool {
    key.to_string_lossy().starts_with(PREFIX)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchdogProtocolError(String);

impl WatchdogProtocolError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for WatchdogProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for WatchdogProtocolError {}

fn pair(key: &str, value: impl Into<OsString>) -> (OsString, OsString) {
    (OsString::from(key), value.into())
}

fn required<'a>(
    values: &'a HashMap<OsString, OsString>,
    key: &str,
) -> Result<&'a std::ffi::OsStr, WatchdogProtocolError> {
    values
        .get(std::ffi::OsStr::new(key))
        .map(OsString::as_os_str)
        .ok_or_else(|| WatchdogProtocolError::new(format!("missing {key}")))
}

fn parse_count(
    values: &HashMap<OsString, OsString>,
    key: &str,
    label: &str,
) -> Result<usize, WatchdogProtocolError> {
    required(values, key)?
        .to_str()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|count| *count <= MAX_ITEMS)
        .ok_or_else(|| WatchdogProtocolError::new(format!("invalid {label}")))
}
