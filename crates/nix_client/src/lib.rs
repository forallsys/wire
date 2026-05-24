#![feature(sync_nonpoison)]
#![feature(nonpoison_mutex)]

use std::sync::atomic::AtomicBool;
use std::{
    borrow::{Borrow, Cow},
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, nonpoison::Mutex},
};

use itertools::Itertools;
use miette::Diagnostic;
use nix_compat::wire::read_string;
use nix_compat::{
    log::VerbosityLevel,
    narinfo::Signature,
    nix_daemon::types::UnkeyedValidPathInfo,
    nixhash::CAHash,
    wire::{
        ProtocolVersion,
        de::{NixRead, NixReader, NixReaderBuilder},
        ser::{NixWrite, NixWriter, NixWriterBuilder},
    },
    worker_protocol::Operation,
};
use nix_compat::{
    log::{ActivityType, Field, LogMessage, ResultType},
    wire::{de::NixDeserialize, ser::NixSerialize},
};
use owo_colors::{OwoColorize, Stream};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, ReadHalf, Take, WriteHalf, split},
    net::UnixStream,
};
use tracing::{Level, debug, info, instrument, trace, warn};

use crate::store_path::SafeStorePath;

pub mod store_path;

// https://snix.dev/docs/reference/nix-daemon-protocol/changelog/
const CLIENT_VERSION: ProtocolVersion = ProtocolVersion::from_parts(1, 35);
const CLIENT_ONE: u64 = 0x6e69_7863;
const SERVER_ONE: u64 = 0x6478_696f;

const STDERR_ERROR: u64 = 0x6378_7470; // "cxtp"
const STDERR_READ: u64 = 0x6461_7461; // "data"
const STDERR_LAST: u64 = 0x616c_7473;
const STDERR_NEXT: u64 = 0x6f6c_6d67;
const STDERR_WRITE: u64 = 0x6461_7416;
const STDERR_START_ACTIVITY: u64 = 0x5354_5254;
const STDERR_STOP_ACTIVITY: u64 = 0x5354_4f50;
const STDERR_RESULT: u64 = 0x5253_4c54;

#[derive(Debug, Diagnostic, Error)]
pub enum NixDaemonClientError {
    #[diagnostic(code(wire::NixDaemonIO))]
    #[error("nix daemon io error")]
    NixDaemonIO(#[source] std::io::Error),

    #[diagnostic(code(wire::NixDaemonInvalidResponse))]
    #[error("nix daemon returned an invalid response: {}", .0)]
    NixDaemonInvalidResponse(String),

    #[diagnostic(code(wire::NixDaemonOperationFailed))]
    #[error("nix daemon operation failed: {}", .0)]
    NixDaemonOperationFailed(String),

    #[diagnostic(code(wire::NixDaemonConnectionFailure))]
    #[error("failed to connect to nix daemon")]
    NixDaemonConnectionFailure(#[source] std::io::Error),

    #[diagnostic(code(wire::NixDaemonProtocolVersion))]
    #[error(
        "the nix daemon protocol version is too old for wire to perform {operation:?}! want atleast {wanted}, have {have}"
    )]
    NixDaemonProtocolVersion {
        wanted: nix_compat::wire::ProtocolVersion,
        have: nix_compat::wire::ProtocolVersion,
        operation: String,
    },

    #[diagnostic(code(wire::NixDaemonOperationError))]
    #[error("{name}: {msg}")]
    NixDaemonOperationError { name: String, msg: String },

    #[diagnostic(code(wire::SIGINT))]
    #[error("SIGINT received, shut down")]
    Sigint,
}

// `AddToStoreNar` with SafeSTorePath & a nar_hash which is a string instead of
// a CAHash (which cannot be easily deserialized and serialised)
#[derive(Debug)]
pub struct WireAddToStoreNarRequest {
    pub path: SafeStorePath<String>,
    pub deriver: Option<SafeStorePath<String>>,
    pub nar_hash: String,
    pub references: Vec<SafeStorePath<String>>,
    pub registration_time: u64,
    pub nar_size: u64,
    pub ultimate: bool,
    pub signatures: Vec<Signature<String>>,
    pub ca: Option<CAHash>,
    pub repair: bool,
    pub dont_check_sigs: bool,
}

#[derive(Debug)]
pub struct QueryMissingResult {
    will_build: HashSet<SafeStorePath<String>>,
    will_substitute: HashSet<SafeStorePath<String>>,
    _unknown: HashSet<SafeStorePath<String>>,
    download_size: u64,
    _nar_size: u64,
}

#[derive(Debug)]
pub enum DerivedPathOutput<'a, S: Borrow<str> + std::fmt::Debug> {
    // wont support `*` as wire does not require that yet and some protocols
    // dont have it.
    // /// All (*) outputs
    // All,
    /// List of output names
    OutputNames(&'a [S]),
}

#[derive(Debug)]
pub struct DerivedPath<'a, S: Borrow<str> + std::fmt::Debug> {
    pub store_path: &'a SafeStorePath<String>,
    pub outputs: DerivedPathOutput<'a, S>,
}

pub struct NixClient<R, W, T> {
    reader: NixReader<R>,
    writer: NixWriter<W>,

    build_name_map: Arc<Mutex<HashMap<u64, Arc<String>>>>,
    should_quit: Arc<AtomicBool>,
    print_build_logs: bool,

    trace_callback: T,
}

impl<T> NixClient<UnixStream, UnixStream, T>
where
    T: Fn(LogMessage, &Arc<Mutex<HashMap<u64, Arc<String>>>>, bool) -> Option<String>,
{
    #[instrument(skip(trace_callback))]
    pub async fn open_local(
        trace_callback: T,
        should_quit: Arc<AtomicBool>,
        print_build_logs: bool,
    ) -> Result<NixClient<ReadHalf<UnixStream>, WriteHalf<UnixStream>, T>, NixDaemonClientError>
    {
        let stream = UnixStream::connect("/nix/var/nix/daemon-socket/socket")
            .await
            .map_err(NixDaemonClientError::NixDaemonConnectionFailure)?;

        let (reader, writer) = split(stream);

        NixClient::<ReadHalf<UnixStream>, WriteHalf<UnixStream>, T>::handshake(
            reader,
            writer,
            trace_callback,
            should_quit,
            print_build_logs,
        )
        .await
    }
}

impl<R, W, T> NixClient<R, W, T>
where
    T: Fn(LogMessage, &Arc<Mutex<HashMap<u64, Arc<String>>>>, bool) -> Option<String>,
{
    #[instrument(skip_all)]
    pub async fn handshake(
        mut reader: R,
        mut writer: W,
        trace_callback: T,
        should_quit: Arc<AtomicBool>,
        print_build_logs: bool,
    ) -> Result<NixClient<R, W, T>, NixDaemonClientError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
    {
        trace!("sending {CLIENT_ONE:x?} in handshake");

        writer
            .write_u64_le(CLIENT_ONE)
            .await
            .map_err(NixDaemonClientError::NixDaemonIO)?;

        let magic = reader
            .read_u64_le()
            .await
            .map_err(NixDaemonClientError::NixDaemonIO)?;

        trace!("server responded with magic {magic:x?}");

        if magic != SERVER_ONE {
            return Err(NixDaemonClientError::NixDaemonInvalidResponse(format!(
                "daemon returned invalid magic in handshake: {magic:?}"
            )));
        }

        let protocol_version: u64 = reader
            .read_u64_le()
            .await
            .map_err(NixDaemonClientError::NixDaemonIO)?;

        trace!(server_version = ?protocol_version);

        let server_version: nix_compat::wire::ProtocolVersion =
            protocol_version.try_into().map_err(|error: &str| {
                NixDaemonClientError::NixDaemonInvalidResponse(error.to_string())
            })?;

        // match server's protocol version
        writer
            .write_u64_le(CLIENT_VERSION.into())
            .await
            .map_err(NixDaemonClientError::NixDaemonIO)?;

        // write obsolete `sendCpu` & `cpuAffinity`
        writer
            .write_u64_le(0)
            .await
            .map_err(NixDaemonClientError::NixDaemonIO)?;
        writer
            .write_u64_le(0)
            .await
            .map_err(NixDaemonClientError::NixDaemonIO)?;

        let nix_version = read_string(&mut reader, 0..=10)
            .await
            .map_err(NixDaemonClientError::NixDaemonIO)?;

        let trusted = reader
            .read_u64_le()
            .await
            .map_err(NixDaemonClientError::NixDaemonIO)?;

        debug!(daemon_nix_version = ?nix_version, server_version = ?server_version, trusted = ?trusted, "completed handshake with daemon");

        let reader = NixReaderBuilder::default()
            .set_version(server_version)
            .build(reader);
        let writer = NixWriterBuilder::default()
            .set_version(server_version)
            .build(writer);

        let mut result = Self {
            reader,
            writer,
            build_name_map: Arc::new(Mutex::new(HashMap::new())),
            trace_callback,
            should_quit,
            print_build_logs,
        };

        result.drain_stderr().await?;

        return Ok(result);
    }

    #[instrument(level = Level::TRACE, skip_all, ret)]
    async fn read_value<V>(&mut self) -> Result<V, NixDaemonClientError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        V: NixDeserialize + std::fmt::Debug,
    {
        self.shutdown_guard()?;

        self.reader
            .read_value::<V>()
            .await
            .map_err(NixDaemonClientError::NixDaemonIO)
    }

    #[instrument(level = Level::TRACE, skip(self), ret)]
    async fn write_value<V>(&mut self, value: &V) -> Result<(), NixDaemonClientError>
    where
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
        V: NixSerialize + std::fmt::Debug + Send,
    {
        self.shutdown_guard()?;

        self.writer
            .write_value(value)
            .await
            .map_err(NixDaemonClientError::NixDaemonIO)
    }

    #[instrument(skip(self))]
    async fn read_error(&mut self) -> Result<NixDaemonClientError, NixDaemonClientError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
    {
        let _type: String = self.read_value().await?;
        let _level: VerbosityLevel = self.read_value().await?;
        let name: String = self.read_value().await?;
        let msg: String = self.read_value().await?;
        let _have_pos: u64 = self.read_value().await?;

        Ok(NixDaemonClientError::NixDaemonOperationError { name, msg })
    }

    fn shutdown_guard(&self) -> Result<(), NixDaemonClientError> {
        if self.should_quit.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(NixDaemonClientError::Sigint);
        }

        Ok(())
    }

    #[instrument(skip(self))]
    async fn drain_stderr(&mut self) -> Result<(), NixDaemonClientError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
    {
        loop {
            self.shutdown_guard()?;

            let msg_type: u64 = self.read_value().await?;

            match msg_type {
                STDERR_LAST => {
                    trace!("stderr stream ended normally");
                    break;
                }
                STDERR_ERROR => {
                    debug!("stderr error encountered");
                    return Err(self.read_error().await?);
                }
                STDERR_NEXT => {
                    let msg = self.read_value().await?;

                    // normal string log message
                    (self.trace_callback)(
                        LogMessage::Msg {
                            level: VerbosityLevel::Info,
                            msg: Cow::Owned(msg),
                        },
                        &self.build_name_map,
                        self.print_build_logs,
                    );
                }
                STDERR_START_ACTIVITY => {
                    let activity = self.read_activity_start().await?;

                    (self.trace_callback)(activity, &self.build_name_map, self.print_build_logs);
                }
                STDERR_STOP_ACTIVITY => {
                    let id = self.read_value().await?;
                    (self.trace_callback)(
                        LogMessage::Stop { id },
                        &self.build_name_map,
                        self.print_build_logs,
                    );
                }
                STDERR_RESULT => {
                    let id: u64 = self.read_value().await?;
                    let result_type: ResultType = ResultType::try_from(
                        u8::try_from(self.read_value::<u64>().await?).map_err(|err| {
                            NixDaemonClientError::NixDaemonInvalidResponse(format!(
                                "could not cast result type to u8: {err:?}"
                            ))
                        })?,
                    )
                    .map_err(|err| {
                        NixDaemonClientError::NixDaemonInvalidResponse(format!(
                            "could not convert u64 to ResultType: {err:?}"
                        ))
                    })?;

                    let fields = self.read_activity_fields().await?;

                    (self.trace_callback)(
                        LogMessage::Result {
                            fields,
                            id,
                            r#type: result_type,
                        },
                        &self.build_name_map,
                        self.print_build_logs,
                    );
                }
                STDERR_READ => {
                    let _desired_len: u64 = self.read_value().await?;

                    warn!("STDERR_READ is not implemented");
                }
                STDERR_WRITE => {
                    // todo: read bytes here
                    warn!("STDERR_WRITE is not implemented");
                }
                _ => {
                    return Err(NixDaemonClientError::NixDaemonInvalidResponse(format!(
                        "unknown daemon message type in stderr stream: {msg_type:x}"
                    )));
                }
            }
        }

        Ok(())
    }

    // https://snix.dev/docs/reference/nix-daemon-protocol/types/#field
    #[instrument(skip(self))]
    async fn read_activity_start<'a>(&mut self) -> Result<LogMessage<'a>, NixDaemonClientError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
    {
        let id: u64 = self.read_value().await?;
        let level: VerbosityLevel = self.read_value().await?;

        let activity_type: ActivityType = ActivityType::try_from(
            u8::try_from(self.read_value::<u64>().await?).map_err(|err| {
                NixDaemonClientError::NixDaemonInvalidResponse(format!(
                    "could not cast activity type to u8: {err:?}"
                ))
            })?,
        )
        .map_err(|err| {
            NixDaemonClientError::NixDaemonInvalidResponse(format!(
                "could not convert u64 to ActivityType: {err:?}"
            ))
        })?;

        let text: String = self.read_value().await?;

        let fields = self.read_activity_fields().await?;

        let parent: u64 = self.read_value().await?;

        return Ok(LogMessage::Start {
            fields: Some(fields),
            id,
            level,
            parent,
            text: text.into(),
            r#type: activity_type,
        });
    }

    // https://snix.dev/docs/reference/nix-daemon-protocol/types/#field
    #[instrument(skip(self))]
    async fn read_activity_fields<'a>(&mut self) -> Result<Vec<Field<'a>>, NixDaemonClientError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
    {
        let fields_count: u64 = self.read_value().await?;

        let mut fields = Vec::new();

        for _ in 0..fields_count {
            let field_type: u64 = self.read_value().await?;

            match field_type {
                0 => {
                    fields.push(Field::Int(self.read_value().await?));
                }
                1 => {
                    let str_val: String = self.read_value().await?;

                    fields.push(Field::String(Cow::Owned(str_val.into_bytes())));
                }
                _ => {
                    return Err(NixDaemonClientError::NixDaemonInvalidResponse(format!(
                        "unknown activity field type: {field_type}",
                    )));
                }
            }
        }

        Ok(fields)
    }

    /// Takes a list of store paths and returns a new list only containing the valid store paths
    /// more information: <https://snix.dev/docs/reference/nix-daemon-protocol/operations/#queryvalidpaths>
    #[instrument(skip_all)]
    pub async fn query_valid_paths(
        &mut self,
        paths: Vec<SafeStorePath<String>>,
        substitute: bool,
    ) -> Result<Vec<SafeStorePath<String>>, NixDaemonClientError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
    {
        self.write_value(&Operation::QueryValidPaths).await?;
        self.write_value(&paths).await?;

        // write `substitute` bool
        // https://snix.dev/docs/reference/nix-daemon-protocol/operations/#if-protocol-version-is-127-or-newer
        if self.writer.version() >= ProtocolVersion::from_parts(1, 27) {
            self.write_value(&substitute).await?;
        }

        self.writer
            .flush()
            .await
            .map_err(NixDaemonClientError::NixDaemonIO)?;

        self.drain_stderr().await?;

        #[allow(clippy::disallowed_types)]
        let valid_paths: Vec<nix_compat::store_path::StorePath<String>> = self.read_value().await?;

        return Ok(valid_paths.into_iter().map(SafeStorePath).collect());
    }

    #[instrument(skip(self))]
    #[allow(clippy::disallowed_types)]
    pub async fn collect_complete_closure(
        &mut self,
        root_path: &SafeStorePath<String>,
    ) -> Result<Vec<SafeStorePath<String>>, NixDaemonClientError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
    {
        let mut graph = HashMap::new();
        let mut queue = vec![root_path.clone()];

        while let Some(current_path) = queue.pop() {
            if graph.contains_key(&current_path) {
                continue;
            }

            // debug!(path = ?current_path, "querying path");

            let path_info = self.query(&current_path).await?;

            let Some(path_info) = path_info else {
                return Err(NixDaemonClientError::NixDaemonOperationFailed(format!(
                    "{current_path:?} does not exist in store"
                )));
            };

            graph.insert(
                current_path.clone(),
                path_info
                    .references
                    .clone()
                    .into_iter()
                    .map(SafeStorePath)
                    .collect(),
            );

            for reference in path_info.references {
                let reference = SafeStorePath(reference);

                if !graph.contains_key(&reference) {
                    queue.push(reference);
                }
            }
        }

        let mut visited = HashSet::new();
        let mut ordered = Vec::new();

        // https://en.wikipedia.org/wiki/Topological_sorting#Depth-first_search
        fn visit(
            path: &SafeStorePath<String>,
            graph: &HashMap<SafeStorePath<String>, Vec<SafeStorePath<String>>>,
            visited: &mut HashSet<SafeStorePath<String>>,
            ordered: &mut Vec<SafeStorePath<String>>,
        ) {
            if visited.contains(path) {
                return;
            }

            visited.insert(path.clone());

            if let Some(refs) = graph.get(path) {
                for reference in refs {
                    visit(reference, graph, visited, ordered);
                }
            }

            ordered.push(path.clone());
        }

        visit(root_path, &graph, &mut visited, &mut ordered);

        Ok(ordered)
    }

    // https://snix.dev/docs/reference/nix-daemon-protocol/operations/#querypathinfo
    #[instrument(skip(self))]
    #[allow(clippy::disallowed_types)]
    pub async fn query(
        &mut self,
        path: &SafeStorePath<String>,
    ) -> Result<Option<UnkeyedValidPathInfo>, NixDaemonClientError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
    {
        if self.writer.version() < ProtocolVersion::from_parts(1, 17) {
            return Err(NixDaemonClientError::NixDaemonProtocolVersion {
                wanted: ProtocolVersion::from_parts(1, 17),
                have: self.writer.version(),
                operation: "QueryPathInfo".into(),
            });
        }

        self.write_value(&Operation::QueryPathInfo).await?;
        self.write_value(path).await?;
        self.writer
            .flush()
            .await
            .map_err(NixDaemonClientError::NixDaemonIO)?;

        self.drain_stderr().await?;

        let success: bool = self.read_value().await?;

        if !success {
            return Ok(None);
        }

        // #[allow(clippy::disallowed_types)]
        // let deriver: String = self
        //     .reader
        //     .read_value()
        //     .await
        //     .map_err(NixDaemonClientError::NixDaemonIO)?;
        //
        // info!("{deriver:?}");

        #[allow(clippy::disallowed_types)]
        let deriver: Option<nix_compat::store_path::StorePath<String>> = self.read_value().await?;

        let nar_hash: String = self.read_value().await?;

        #[allow(clippy::disallowed_types)]
        let references: Vec<nix_compat::store_path::StorePath<String>> = self.read_value().await?;

        let registration_time: u64 = self.read_value().await?;

        let nar_size: u64 = self.read_value().await?;

        let (ultimate, signatures, ca) =
            if self.writer.version() >= ProtocolVersion::from_parts(1, 16) {
                let ultimate: bool = self.read_value().await?;

                let signatures: Vec<Signature<String>> = self.read_value().await?;

                let ca: String = self.read_value().await?;

                (
                    ultimate,
                    signatures,
                    if ca.is_empty() {
                        None
                    } else {
                        CAHash::from_nix_hex_str(&ca)
                    },
                )
            } else {
                (false, Vec::new(), None)
            };

        return Ok(Some(UnkeyedValidPathInfo {
            deriver,
            nar_hash,
            references,
            registration_time,
            nar_size,
            ultimate,
            signatures,
            ca,
        }));
    }

    #[instrument(skip(self))]
    #[allow(clippy::disallowed_types)]
    pub async fn get_nar_stream(
        &mut self,
        path: &SafeStorePath<String>,
        nar_size: u64,
    ) -> Result<Take<&mut NixReader<R>>, NixDaemonClientError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
    {
        if self.writer.version() < ProtocolVersion::from_parts(1, 17) {
            return Err(NixDaemonClientError::NixDaemonProtocolVersion {
                wanted: ProtocolVersion::from_parts(1, 17),
                have: self.writer.version(),
                operation: "NarFromPath".into(),
            });
        }

        self.write_value(&Operation::NarFromPath).await?;
        self.write_value(&path).await?;

        self.writer
            .flush()
            .await
            .map_err(NixDaemonClientError::NixDaemonIO)?;
        self.drain_stderr().await?;

        Ok((&mut self.reader).take(nar_size))
    }

    #[instrument(skip(self, stream))]
    #[allow(clippy::disallowed_types)]
    pub async fn framed_write<RS>(&mut self, mut stream: RS) -> Result<(), NixDaemonClientError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
        RS: AsyncReadExt + Unpin + Send,
    {
        // 64KB buffer
        let mut buffer = vec![0u8; 64 * 1024];

        loop {
            let bytes_read = stream
                .read(&mut buffer)
                .await
                .map_err(NixDaemonClientError::NixDaemonIO)?;

            if bytes_read == 0 {
                // write a zero length to indicate the end of the framed stream
                self.write_value(&0u64).await?;

                break;
            }

            // write the chunk size
            self.write_value(&(bytes_read as u64)).await?;

            self.writer
                .write_all(&buffer[..bytes_read])
                .await
                .map_err(NixDaemonClientError::NixDaemonIO)?;
        }

        self.writer
            .flush()
            .await
            .map_err(NixDaemonClientError::NixDaemonIO)?;

        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn query_derivation_output_map(
        &mut self,
        store_path: &SafeStorePath<String>,
    ) -> Result<BTreeMap<String, Option<SafeStorePath<String>>>, NixDaemonClientError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
    {
        if self.writer.version() < ProtocolVersion::from_parts(1, 22) {
            return Err(NixDaemonClientError::NixDaemonProtocolVersion {
                wanted: ProtocolVersion::from_parts(1, 19),
                have: self.writer.version(),
                operation: "QueryMissing".into(),
            });
        }

        self.write_value(&Operation::QueryDerivationOutputMap)
            .await?;
        self.write_value(&store_path).await?;
        self.writer
            .flush()
            .await
            .map_err(NixDaemonClientError::NixDaemonIO)?;

        self.drain_stderr().await?;

        #[allow(clippy::disallowed_types)]
        let map: BTreeMap<String, Option<nix_compat::store_path::StorePath<String>>> =
            self.read_value().await?;

        return Ok(map
            .into_iter()
            .map(|(key, value)| (key, value.map(SafeStorePath)))
            .collect());
    }

    #[instrument(skip(self))]
    #[allow(clippy::disallowed_types)]
    pub async fn query_missing<S>(
        &mut self,
        derived_path: &Vec<DerivedPath<'_, S>>,
    ) -> Result<QueryMissingResult, NixDaemonClientError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
        S: Borrow<str> + std::fmt::Debug + Sync,
    {
        if self.writer.version() < ProtocolVersion::from_parts(1, 19) {
            return Err(NixDaemonClientError::NixDaemonProtocolVersion {
                wanted: ProtocolVersion::from_parts(1, 19),
                have: self.writer.version(),
                operation: "QueryMissing".into(),
            });
        }

        self.write_value(&Operation::QueryMissing).await?;
        self.write_value(derived_path).await?;
        self.writer
            .flush()
            .await
            .map_err(NixDaemonClientError::NixDaemonIO)?;

        self.drain_stderr().await?;

        self.read_value().await
    }

    #[instrument(skip(self))]
    #[allow(clippy::disallowed_types)]
    pub async fn build<S>(
        &mut self,
        derived_paths: &Vec<DerivedPath<'_, S>>,
    ) -> Result<(), NixDaemonClientError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
        S: Borrow<str> + std::fmt::Debug + Sync,
    {
        if self.writer.version() < ProtocolVersion::from_parts(1, 19) {
            info!(
                "daemon does not support QueryMissing, cannot log which paths will be built or substituted"
            );
        } else {
            let missing = self.query_missing(derived_paths).await?;

            trace!("missing: {missing:?}");

            if !missing.will_build.is_empty() {
                info!(
                    "The following {} paths will be built: \n\t{}",
                    missing
                        .will_build
                        .len()
                        .if_supports_color(Stream::Stderr, |x| x.green()),
                    missing
                        .will_build
                        .into_iter()
                        .map(|x| x.to_absolute_path())
                        .join("\n\t")
                );
            }

            if !missing.will_substitute.is_empty() {
                info!(
                    "{} paths will be substituted for {} bytes: \n\t{}",
                    missing
                        .will_substitute
                        .len()
                        .if_supports_color(Stream::Stderr, |x| x.green()),
                    missing.download_size,
                    missing
                        .will_substitute
                        .into_iter()
                        .map(|x| x.to_absolute_path())
                        .join("\n\t"),
                );
            }
        }

        self.write_value(&Operation::BuildPaths).await?;
        self.write_value(derived_paths).await?;

        if self.writer.version() >= ProtocolVersion::from_parts(1, 15) {
            self.write_value(&0u64).await?;
        }

        self.writer
            .flush()
            .await
            .map_err(NixDaemonClientError::NixDaemonIO)?;

        self.drain_stderr().await?;

        let value = self.read_value::<u64>().await?;

        trace!("end of build: read u64 {value:?}");

        Ok(())
    }

    #[instrument(skip(self, nar_stream))]
    #[allow(clippy::disallowed_types)]
    pub async fn add_to_store_nar<RS>(
        &mut self,
        data: WireAddToStoreNarRequest,
        nar_stream: RS,
    ) -> Result<(), NixDaemonClientError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
        RS: AsyncReadExt + Unpin + Send,
    {
        // non-framed data writes are not implemented
        if self.writer.version() < ProtocolVersion::from_parts(1, 23) {
            return Err(NixDaemonClientError::NixDaemonProtocolVersion {
                wanted: ProtocolVersion::from_parts(1, 23),
                have: self.writer.version(),
                operation: "AddToStoreNar".into(),
            });
        }

        self.write_value(&Operation::AddToStoreNar).await?;
        self.write_value(&data).await?;

        self.framed_write(nar_stream).await?;
        self.drain_stderr().await?;

        Ok(())
    }
}

impl<S: Borrow<str> + std::fmt::Debug + Sync> NixSerialize for DerivedPath<'_, S> {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        writer.write_value(&self).await
    }
}

// https://snix.dev/docs/reference/nix-daemon-protocol/types/#derivedpath
impl<S: Borrow<str> + std::fmt::Debug + Sync> NixSerialize for &DerivedPath<'_, S> {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        let output_names = match self.outputs {
            DerivedPathOutput::OutputNames(outputs) => outputs.join(","),
        };

        let output_spec = format!("{}!{}", self.store_path.to_absolute_path(), output_names);

        writer.write_value(&output_spec).await
    }
}

impl NixSerialize for WireAddToStoreNarRequest {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        writer.write_value(&self.path).await?;
        #[allow(clippy::disallowed_types)]
        writer
            .write_value(
                &self
                    .deriver
                    .clone()
                    .map(Into::<nix_compat::store_path::StorePath<String>>::into),
            )
            .await?;
        writer.write_value(&self.nar_hash).await?;
        writer.write_value(&self.references).await?;
        writer.write_value(&self.registration_time).await?;
        writer.write_value(&self.nar_size).await?;
        writer.write_value(&self.ultimate).await?;
        writer.write_value(&self.signatures).await?;
        writer.write_value(&self.ca).await?;
        writer.write_value(&self.repair).await?;
        writer.write_value(&self.dont_check_sigs).await
    }
}

impl NixDeserialize for QueryMissingResult {
    async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
    where
        R: ?Sized + NixRead + Send,
    {
        let will_build: Option<Vec<SafeStorePath<String>>> = reader.try_read_value().await?;
        let will_substitute: Option<Vec<SafeStorePath<String>>> = reader.try_read_value().await?;
        let unknown: Option<Vec<SafeStorePath<String>>> = reader.try_read_value().await?;
        let download_size = reader.try_read_number().await?;
        let nar_size = reader.try_read_number().await?;

        if let Some(will_build) = will_build
            && let Some(will_substitute) = will_substitute
            && let Some(unknown) = unknown
            && let Some(download_size) = download_size
            && let Some(nar_size) = nar_size
        {
            Ok(Some(QueryMissingResult {
                will_build: will_build.into_iter().collect(),
                will_substitute: will_substitute.into_iter().collect(),
                _unknown: unknown.into_iter().collect(),
                download_size,
                _nar_size: nar_size,
            }))
        } else {
            Ok(None)
        }
    }
}
