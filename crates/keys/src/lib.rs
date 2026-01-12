use secrecy::{ExposeSecret, SecretSlice};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Cursor, path::PathBuf, pin::Pin, process::Stdio, str::from_utf8,
};
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};
use wire_core::{
    errors::KeyError,
    hive::steps::keys::{Source, UploadKeyAt},
};

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Hash)]
pub struct Key {
    pub name: String,
    #[serde(rename = "destDir")]
    pub dest_dir: String,
    pub path: PathBuf,
    pub group: String,
    pub user: String,
    pub permissions: String,
    pub source: Source,
    #[serde(rename = "uploadAt")]
    pub upload_at: UploadKeyAt,
    #[serde(default)]
    pub environment: im::HashMap<String, String>,
}

// /// what nodes hold in execution
// #[derive(Debug)]
// pub struct StoredKey {
//     pub key: Key,
//
//     data: Arc<SecretSlice<u8>>,
// }
//
// #[derive(Default)]
// pub struct KeyStore {
//     // keys: HashSet<Arc<Key>>,
//     // keys: HashMap<Arc<Key>, Option<Arc<Vec<u8>>>>,
//
//     cache: DashMap<Key, Weak<OnceCell<SecretSlice<u8>>>>
// }

// impl KeyStore {
//     #[must_use]
//     pub fn new() -> Self {
//         Self { cache: DashMap::new() }
//     }
//
//     // pub fn insert(&mut self, key: Key) -> Arc<Key> {
//     //     let key = Arc::new(key);
//     //
//     //     self.keys.entry(key.clone()).or_insert(None);
//     //
//     //     if self.keys.contains_key(&key) {
//     //         return self.keys.get_key_value(&key).unwrap().0.clone()
//     //     }
//     //
//     //     let value = Arc::new(key);
//     //     self.keys.insert(value.clone(), None);
//     //
//     //     value
//     // }
//     //
//     // pub async fn read(&mut self, key: Arc<Key>) -> Result<&Vec<u8>, KeyError> {
//     //     if let Some(Some(value)) = self.keys.get(&key) {
//     //         return Ok(value);
//     //     }
//     //
//     //     let mut buf = Vec::new();
//     //
//     //     let mut reader = key.create_reader().await?;
//     //
//     //     reader
//     //         .read_to_end(&mut buf)
//     //         .await
//     //         .expect("failed to read into buffer");
//     //
//     //     drop(reader);
//     //
//     //     self.keys.insert(key.clone(), Some(buf));
//     //
//     //     Ok(self.keys.get(&key).unwrap().as_ref().unwrap())
//     //
//     // }
// }

impl Key {
    async fn create_reader(&self) -> Result<Pin<Box<dyn AsyncRead + Send + '_>>, KeyError> {
        match &self.source {
            Source::Path(path) => Ok(Box::pin(File::open(path).await.map_err(KeyError::File)?)),
            Source::String(string) => Ok(Box::pin(Cursor::new(string))),
            Source::Command(args) => {
                let output = Command::new(args.first().ok_or(KeyError::Empty)?)
                    .args(&args[1..])
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .envs(self.environment.clone())
                    .spawn()
                    .map_err(|err| KeyError::CommandSpawnError {
                        error: err,
                        command: args.join(" "),
                        command_span: Some((0..args.first().unwrap().len()).into()),
                    })?
                    .wait_with_output()
                    .await
                    .map_err(|err| KeyError::CommandResolveError {
                        error: err,
                        command: args.join(" "),
                    })?;

                if output.status.success() {
                    return Ok(Box::pin(Cursor::new(output.stdout)));
                }

                Err(KeyError::CommandError(
                    output.status,
                    from_utf8(&output.stderr).unwrap().to_string(),
                ))
            }
        }
    }

    fn get_u32_unix_mode(&self) -> Result<u32, KeyError> {
        u32::from_str_radix(&self.permissions, 8).map_err(KeyError::ParseKeyPermissions)
    }


    pub async fn read(&self) -> Result<(wire_key_agent::keys::KeySpec, SecretSlice<u8>), KeyError> {
        let mut buf = Vec::new();
        let mut reader = self.create_reader().await?;

        reader
            .read_to_end(&mut buf)
            .await
            .expect("failed to read into buffer");

        let buf = SecretSlice::from(buf);

        let destination: PathBuf = [self.dest_dir.clone(), self.name.clone()].iter().collect();

        Ok((
        wire_key_agent::keys::KeySpec {
            length: buf
                .expose_secret()
                .len()
                .try_into()
                .expect("Failed to convert usize buf length to i32"),
            user: self.user.clone(),
            group: self.group.clone(),
            unix_mode: self.get_u32_unix_mode()?,
            destination: destination.into_os_string().into_string().unwrap(),
            digest: Sha256::digest(buf.expose_secret()).to_vec(),
            last: false,
        },
            buf))
    }
}
