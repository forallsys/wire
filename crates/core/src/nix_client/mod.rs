use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    ops::Deref,
    process::Stdio,
    sync::{Arc, nonpoison::Mutex},
};

use nix_compat::log::{ActivityType, Field, LogMessage, ResultType};
#[allow(clippy::disallowed_types)]
use nix_compat::{
    log::VerbosityLevel,
    narinfo::Signature,
    nix_daemon::types::UnkeyedValidPathInfo,
    nixhash::CAHash,
    store_path::StorePath,
    wire::{
        ProtocolVersion,
        de::{NixRead, NixReader, NixReaderBuilder},
        read_string,
        ser::{NixWrite, NixWriter, NixWriterBuilder},
    },
    worker_protocol::Operation,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, ReadHalf, Take, WriteHalf, split},
    net::UnixStream,
    process::{ChildStdin, ChildStdout, Command},
};
use tracing::{debug, info, instrument, trace};

use crate::{
    SafeStorePath, SubCommandModifiers, commands::trace_nix_log_message, errors::HiveLibError,
    hive::node::BuildNameMap,
};

pub mod utils;

// https://snix.dev/docs/reference/nix-daemon-protocol/handshake
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

#[derive(Debug)]
pub(crate) struct WireAddToStoreNarRequest {
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

pub(crate) struct NixClient<R, W> {
    reader: NixReader<R>,
    writer: NixWriter<W>,

    build_name_map: BuildNameMap,
}

impl NixClient<UnixStream, UnixStream> {
    #[instrument]
    pub(crate) async fn open_local()
    -> Result<NixClient<ReadHalf<UnixStream>, WriteHalf<UnixStream>>, HiveLibError> {
        let stream = UnixStream::connect("/nix/var/nix/daemon-socket/socket")
            .await
            .map_err(HiveLibError::NixDaemonConnectionFailure)?;

        let (reader, writer) = split(stream);

        NixClient::<ReadHalf<UnixStream>, WriteHalf<UnixStream>>::handshake(reader, writer).await
    }
}

impl NixClient<ChildStdin, ChildStdout> {
    #[instrument]
    pub(crate) async fn open_remote<D>(
        target: &D,
        modifiers: SubCommandModifiers,
    ) -> Result<(NixClient<ChildStdout, ChildStdin>, String), HiveLibError>
    where
        D: Deref<Target = crate::hive::node::Target> + std::fmt::Debug,
    {
        let mut command = Command::new("ssh")
            .args(target.create_ssh_args(modifiers, true)?)
            .arg(target.get_preferred_host()?.to_string())
            .arg("nix-daemon --stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // TODO: move to separate thread
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(HiveLibError::NixDaemonConnectionFailure)?;

        let stdin = command.stdin.take().unwrap();
        let stdout = command.stdout.take().unwrap();

        Ok((
            NixClient::<ChildStdout, ChildStdin>::handshake(stdout, stdin).await?,
            target.get_preferred_host()?.to_string(),
        ))
    }
}

impl<R, W> NixClient<R, W> {
    #[instrument(skip_all)]
    pub(crate) async fn handshake(
        mut reader: R,
        mut writer: W,
    ) -> Result<NixClient<R, W>, HiveLibError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
    {
        trace!("sending {CLIENT_ONE:x?} in handshake");

        writer
            .write_u64_le(CLIENT_ONE)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        let magic = reader
            .read_u64_le()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        trace!("server responded with magic {magic:x?}");

        if magic != SERVER_ONE {
            return Err(HiveLibError::NixDaemonInvalidResponse(format!(
                "daemon returned invalid magic in handshake: {magic:?}"
            )));
        }

        let protocol_version: u64 = reader
            .read_u64_le()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        trace!(server_version = ?protocol_version);

        let server_version: nix_compat::wire::ProtocolVersion = protocol_version
            .try_into()
            .map_err(|error: &str| HiveLibError::NixDaemonInvalidResponse(error.to_string()))?;

        // match server's protocol version
        writer
            .write_u64_le(protocol_version)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        // write obsolete `sendCpu` & `cpuAffinity`
        writer
            .write_u64_le(0)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        writer
            .write_u64_le(0)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        let nix_version = read_string(&mut reader, 0..=10)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        let trusted = reader
            .read_u64_le()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

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
        };

        result.drain_stderr().await?;

        return Ok(result);
    }

    #[instrument(skip(self))]
    async fn read_error(&mut self) -> Result<HiveLibError, HiveLibError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
    {
        let _type: String = self
            .reader
            .read_value()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        let _level: VerbosityLevel = self
            .reader
            .read_value()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        let name: String = self
            .reader
            .read_value()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        let msg: String = self
            .reader
            .read_value()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        let _have_pos: u64 = self
            .reader
            .read_value()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        Ok(HiveLibError::NixDaemonOperationError { name, msg })
    }

    #[instrument(skip(self))]
    async fn drain_stderr(&mut self) -> Result<(), HiveLibError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
    {
        loop {
            let msg_type: u64 = self
                .reader
                .read_value()
                .await
                .map_err(HiveLibError::NixDaemonIO)?;

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
                    // normal string log message
                    trace_nix_log_message(
                        LogMessage::Msg {
                            level: VerbosityLevel::Info,
                            msg: Cow::Owned(
                                self.reader
                                    .read_value()
                                    .await
                                    .map_err(HiveLibError::NixDaemonIO)?,
                            ),
                        },
                        &self.build_name_map,
                    );
                }
                STDERR_START_ACTIVITY => {
                    trace_nix_log_message(self.read_activity_start().await?, &self.build_name_map);
                }
                STDERR_STOP_ACTIVITY => {
                    trace_nix_log_message(
                        LogMessage::Stop {
                            id: self
                                .reader
                                .read_value()
                                .await
                                .map_err(HiveLibError::NixDaemonIO)?,
                        },
                        &self.build_name_map,
                    );
                }
                STDERR_RESULT => {
                    let id: u64 = self
                        .reader
                        .read_value()
                        .await
                        .map_err(HiveLibError::NixDaemonIO)?;
                    let result_type: ResultType = ResultType::try_from(
                        u8::try_from(
                            self.reader
                                .read_value::<u64>()
                                .await
                                .map_err(HiveLibError::NixDaemonIO)?,
                        )
                        .map_err(|err| {
                            HiveLibError::NixDaemonInvalidResponse(format!(
                                "could not cast result type to u8: {err:?}"
                            ))
                        })?,
                    )
                    .map_err(|err| {
                        HiveLibError::NixDaemonInvalidResponse(format!(
                            "could not convert u64 to ResultType: {err:?}"
                        ))
                    })?;

                    let fields = self.read_activity_fields().await?;

                    trace_nix_log_message(
                        LogMessage::Result {
                            fields,
                            id,
                            r#type: result_type,
                        },
                        &self.build_name_map,
                    );
                }
                STDERR_READ => {
                    let _desired_len: u64 = self
                        .reader
                        .read_value()
                        .await
                        .map_err(HiveLibError::NixDaemonIO)?;

                    unimplemented!("STDERR_READ is not implemented")
                }
                STDERR_WRITE => {
                    // todo: read bytes here
                    unimplemented!("STDERR_WRITE is not implemented")
                }
                _ => {
                    return Err(HiveLibError::NixDaemonInvalidResponse(format!(
                        "unknown daemon message type in stderr stream: {msg_type:x}"
                    )));
                }
            }
        }

        Ok(())
    }

    // https://snix.dev/docs/reference/nix-daemon-protocol/types/#field
    #[instrument(skip(self))]
    async fn read_activity_start<'a>(&mut self) -> Result<LogMessage<'a>, HiveLibError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
    {
        let id: u64 = self
            .reader
            .read_value()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        let level: VerbosityLevel = self
            .reader
            .read_value()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        let activity_type: ActivityType = ActivityType::try_from(
            u8::try_from(
                self.reader
                    .read_value::<u64>()
                    .await
                    .map_err(HiveLibError::NixDaemonIO)?,
            )
            .map_err(|err| {
                HiveLibError::NixDaemonInvalidResponse(format!(
                    "could not cast activity type to u8: {err:?}"
                ))
            })?,
        )
        .map_err(|err| {
            HiveLibError::NixDaemonInvalidResponse(format!(
                "could not convert u64 to ActivityType: {err:?}"
            ))
        })?;

        let text: String = self
            .reader
            .read_value()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        let fields = self.read_activity_fields().await?;

        let parent: u64 = self
            .reader
            .read_value()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

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
    async fn read_activity_fields<'a>(&mut self) -> Result<Vec<Field<'a>>, HiveLibError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
    {
        let fields_count: u64 = self
            .reader
            .read_value()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        let mut fields = Vec::new();

        for _ in 0..fields_count {
            let field_type: u64 = self
                .reader
                .read_value()
                .await
                .map_err(HiveLibError::NixDaemonIO)?;

            match field_type {
                0 => {
                    fields.push(Field::Int(
                        self.reader
                            .read_value()
                            .await
                            .map_err(HiveLibError::NixDaemonIO)?,
                    ));
                }
                1 => {
                    let str_val: String = self
                        .reader
                        .read_value()
                        .await
                        .map_err(HiveLibError::NixDaemonIO)?;

                    fields.push(Field::String(Cow::Owned(str_val.into_bytes())));
                }
                _ => {
                    return Err(HiveLibError::NixDaemonInvalidResponse(format!(
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
    #[allow(clippy::disallowed_types)]
    pub(crate) async fn query_valid_paths<S: AsRef<str> + Send + Sync>(
        &mut self,
        paths: Vec<SafeStorePath<S>>,
        substitute: bool,
    ) -> Result<Vec<SafeStorePath<String>>, HiveLibError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
    {
        self.writer
            .write_value(&Operation::QueryValidPaths)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        self.writer
            .write_value(&paths)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        // write `substitute` bool
        // https://snix.dev/docs/reference/nix-daemon-protocol/operations/#if-protocol-version-is-127-or-newer
        if self.writer.version() >= ProtocolVersion::from_parts(1, 27) {
            self.writer
                .write_value(&substitute)
                .await
                .map_err(HiveLibError::NixDaemonIO)?;
        }

        self.writer
            .flush()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        self.drain_stderr().await?;

        let valid_paths: Vec<StorePath<String>> = self
            .reader
            .read_value()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        return Ok(valid_paths.into_iter().map(SafeStorePath).collect());
    }

    #[instrument(skip(self))]
    #[allow(clippy::disallowed_types)]
    pub(crate) async fn collect_complete_closure(
        &mut self,
        root_path: &SafeStorePath<String>,
    ) -> Result<Vec<SafeStorePath<String>>, HiveLibError>
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
                return Err(HiveLibError::NixDaemonOperationFailed(format!(
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
    pub(crate) async fn query(
        &mut self,
        path: &SafeStorePath<String>,
    ) -> Result<Option<UnkeyedValidPathInfo>, HiveLibError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
    {
        if self.writer.version() < ProtocolVersion::from_parts(1, 17) {
            return Err(HiveLibError::NixDaemonProtocolVersion {
                wanted: ProtocolVersion::from_parts(1, 17),
                have: self.writer.version(),
                operation: "QueryPathInfo".into(),
            });
        }

        self.writer
            .write_value(&Operation::QueryPathInfo)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        self.writer
            .write_value(path)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        self.writer
            .flush()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        self.drain_stderr().await?;

        let success: bool = self
            .reader
            .read_value()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        if !success {
            return Ok(None);
        }

        // #[allow(clippy::disallowed_types)]
        // let deriver: String = self
        //     .reader
        //     .read_value()
        //     .await
        //     .map_err(HiveLibError::NixDaemonIO)?;
        //
        // info!("{deriver:?}");

        #[allow(clippy::disallowed_types)]
        let deriver: Option<StorePath<String>> = self
            .reader
            .read_value()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        let nar_hash: String = self
            .reader
            .read_value()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        #[allow(clippy::disallowed_types)]
        let references: Vec<StorePath<String>> = self
            .reader
            .read_value()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        let registration_time: u64 = self
            .reader
            .read_value()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        let nar_size: u64 = self
            .reader
            .read_value()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        let (ultimate, signatures, ca) =
            if self.writer.version() >= ProtocolVersion::from_parts(1, 16) {
                let ultimate: bool = self
                    .reader
                    .read_value()
                    .await
                    .map_err(HiveLibError::NixDaemonIO)?;

                let signatures: Vec<Signature<String>> = self
                    .reader
                    .read_value()
                    .await
                    .map_err(HiveLibError::NixDaemonIO)?;

                let ca: String = self
                    .reader
                    .read_value()
                    .await
                    .map_err(HiveLibError::NixDaemonIO)?;

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
    pub(crate) async fn get_nar_stream(
        &mut self,
        path: &SafeStorePath<String>,
        nar_size: u64,
    ) -> Result<Take<&mut NixReader<R>>, HiveLibError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
    {
        if self.writer.version() < ProtocolVersion::from_parts(1, 17) {
            return Err(HiveLibError::NixDaemonProtocolVersion {
                wanted: ProtocolVersion::from_parts(1, 17),
                have: self.writer.version(),
                operation: "NarFromPath".into(),
            });
        }

        self.writer
            .write_value(&Operation::NarFromPath)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        self.writer
            .write_value(&path)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        self.writer
            .flush()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        self.drain_stderr().await?;

        Ok((&mut self.reader).take(nar_size))
    }

    #[instrument(skip(self, stream))]
    #[allow(clippy::disallowed_types)]
    pub(crate) async fn framed_write<RS>(&mut self, mut stream: RS) -> Result<(), HiveLibError>
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
                .map_err(HiveLibError::NixDaemonIO)?;

            if bytes_read == 0 {
                // write a zero length to indicate the end of the framed stream
                self.writer
                    .write_value(&0u64)
                    .await
                    .map_err(HiveLibError::NixDaemonIO)?;

                break;
            }

            // write the chunk size
            self.writer
                .write_value(&(bytes_read as u64))
                .await
                .map_err(HiveLibError::NixDaemonIO)?;

            self.writer
                .write_all(&buffer[..bytes_read])
                .await
                .map_err(HiveLibError::NixDaemonIO)?;
        }

        self.writer
            .flush()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        Ok(())
    }

    #[instrument(skip(self))]
    #[allow(clippy::disallowed_types)]
    pub(crate) async fn build(&mut self, path: &SafeStorePath<String>) -> Result<(), HiveLibError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
    {
        info!("trying to build {path:?}");

        // todo: use QueryMissing to show real feedback

        // see https://snix.dev/docs/reference/nix-daemon-protocol/types/#derivedpath
        let derived_path = if self.writer.version() >= ProtocolVersion::from_parts(1, 30) {
            format!("{}!*", path.to_absolute_path())
        } else {
            format!("{}!out", path.to_absolute_path())
        };

        self.writer
            .write_value(&Operation::BuildPaths)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        self.writer
            .write_value(&vec![derived_path])
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        self.writer
            .write_value(&0u64)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        self.writer
            .flush()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        self.drain_stderr().await?;

        let value = self
            .reader
            .read_value::<u64>()
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        info!("read {value:?}");

        Ok(())
    }

    #[instrument(skip(self, nar_stream))]
    #[allow(clippy::disallowed_types)]
    pub(crate) async fn add_to_store_nar<RS>(
        &mut self,
        data: WireAddToStoreNarRequest,
        nar_stream: RS,
    ) -> Result<(), HiveLibError>
    where
        R: AsyncReadExt + std::fmt::Debug + Unpin + Send,
        W: AsyncWriteExt + std::fmt::Debug + Unpin + Send,
        RS: AsyncReadExt + Unpin + Send,
    {
        // non-framed data writes are not implemented
        if self.writer.version() < ProtocolVersion::from_parts(1, 23) {
            return Err(HiveLibError::NixDaemonProtocolVersion {
                wanted: ProtocolVersion::from_parts(1, 23),
                have: self.writer.version(),
                operation: "AddToStoreNar".into(),
            });
        }

        self.writer
            .write_value(&Operation::AddToStoreNar)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        self.writer
            .write_value(&data.path)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        self.writer
            .write_value(&data.deriver.map(Into::<StorePath<String>>::into))
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        self.writer
            .write_value(&data.nar_hash)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        self.writer
            .write_value(&data.references)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        self.writer
            .write_value(&data.registration_time)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        self.writer
            .write_value(&data.nar_size)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        self.writer
            .write_value(&data.ultimate)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        self.writer
            .write_value(&data.signatures)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        self.writer
            .write_value(&data.ca)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        self.writer
            .write_value(&data.repair)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;
        self.writer
            .write_value(&data.dont_check_sigs)
            .await
            .map_err(HiveLibError::NixDaemonIO)?;

        self.framed_write(nar_stream).await?;
        self.drain_stderr().await?;

        Ok(())
    }
}
