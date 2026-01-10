use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet, io::Cursor, path::PathBuf, pin::Pin, process::Stdio, str::from_utf8,
    sync::Arc,
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

#[derive(Debug, Eq, PartialEq, Hash)]
pub struct StoredKey {
    pub key: Key,
    data: Option<Vec<u8>>,
}

#[derive(Default)]
pub struct KeyStore {
    keys: HashSet<Arc<Key>>,
}

impl KeyStore {
    pub fn insert(&mut self, key: Key) -> Arc<Key> {
        if let Some(value) = self.keys.get(&key) {
            return value.clone();
        }

        let value = Arc::new(key);
        self.keys.insert(value.clone());

        value
    }
}

impl StoredKey {
    async fn create_reader(&self) -> Result<Pin<Box<dyn AsyncRead + Send + '_>>, KeyError> {
        match &self.key.source {
            Source::Path(path) => Ok(Box::pin(File::open(path).await.map_err(KeyError::File)?)),
            Source::String(string) => Ok(Box::pin(Cursor::new(string))),
            Source::Command(args) => {
                let output = Command::new(args.first().ok_or(KeyError::Empty)?)
                    .args(&args[1..])
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .envs(self.key.environment.clone())
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

    async fn read(&mut self) -> Result<&Vec<u8>, KeyError> {
        if let Some(ref value) = self.data {
            return Ok(value);
        }

        let mut buf = Vec::new();

        let mut reader = self.create_reader().await?;

        reader
            .read_to_end(&mut buf)
            .await
            .expect("failed to read into buffer");

        drop(reader);

        self.data = Some(buf);

        Ok(self.data.as_ref().unwrap())
    }
}
