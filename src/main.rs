pub mod album;
pub(crate) mod authentication;
mod cli;
pub(crate) mod network;
pub mod user;

use std::{
    path::PathBuf,
    sync::{Arc, LazyLock},
    time::Duration,
};

use cli::*;

use album::Album;
use futures_util::{FutureExt, StreamExt, TryStreamExt};
use indicatif::{MultiProgress, ProgressBar};
use keyring::Entry;
use reqwest::Url;
use tokio::sync::OnceCell;
use user::{User, UserError};

fn get_username_entry() -> keyring::Result<Entry> {
    Entry::new("catbox-cli", "username")
}

fn get_password_entry() -> keyring::Result<Entry> {
    Entry::new("catbox-cli", "password")
}

pub static USER_INSTANCE: LazyLock<Arc<UserInstance>> =
    LazyLock::new(|| Arc::new(UserInstance::new()));

pub static MULTI_PROGRESS: LazyLock<MultiProgress> = LazyLock::new(MultiProgress::new);

#[derive(Default)]
pub struct UserInstance {
    cache: OnceCell<User>,
}

impl UserInstance {
    pub fn new() -> Self {
        Self {
            cache: OnceCell::new(),
        }
    }
    pub async fn get(&self) -> Result<&User, UserError> {
        self.cache.get_or_try_init(User::new).await
    }
}

pub async fn upload_files(paths: impl AsRef<[PathBuf]> + Send) -> color_eyre::Result<Vec<String>> {
    let user = USER_INSTANCE.get().await?;

    futures_util::stream::iter(paths.as_ref())
        .map(|x| {
            user.upload_file(x.clone())
                .map(move |y| Ok::<_, color_eyre::Report>((x, y?)))
        })
        .buffer_unordered(5)
        .map(|x| {
            let (path, url) = x?;
            MULTI_PROGRESS.println(format!("{}: {url}", path.display()))?;
            Ok(url)
        })
        .try_collect::<Vec<_>>()
        .await
}

pub async fn add_to_album(album: String, files: Vec<String>) -> color_eyre::Result<()> {
    let user = USER_INSTANCE.get().await?;

    let album = {
        if album.contains("catbox.moe") {
            Album::new(Url::parse(&album)?)
        } else {
            Album::new(Url::parse(&format!("https://catbox.moe/c/{album}"))?)
        }
    };

    futures_util::stream::iter(files.into_iter().filter_map(|x| {
        if x.contains("files.catbox.moe") {
            Some(Url::parse(&x).ok()?.path_segments()?.next()?.to_owned())
        } else {
            Some(x)
        }
    }))
    .map(move |x| {
        let album = album.clone();

        let pb = ProgressBar::new_spinner();
        MULTI_PROGRESS.add(pb.clone());

        pb.enable_steady_tick(Duration::from_millis(100));

        pb.set_message(format!("Uploading '{x}' to album"));

        async move {
            let x = user.upload_to_album(&album, &x).await;

            pb.finish_and_clear();

            x
        }
    })
    .buffer_unordered(5)
    .try_collect::<Vec<_>>()
    .await?;
    Ok(())
}
/// Album Control
#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let cli: Cli = argh::from_env();

    match cli.command {
        CliSubCommands::File(FileCommand {
            command: FileSubCommands::Upload(FileUpload { paths }),
        }) => {
            upload_files(paths).await?;
        }
        CliSubCommands::File(FileCommand {
            command: FileSubCommands::List(FileList {}),
        }) => {
            let user = USER_INSTANCE.get().await?;
            let files = user.fetch_uploaded_files().await?;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&files)?);
            } else {
                for (i, x) in files.into_iter().rev().enumerate() {
                    println!("File {}: {x}", i + 1);
                }
            }
        }
        CliSubCommands::Album(AlbumCommand {
            command: AlbumSubCommands::Add(AddFiles { album, files }),
        }) => {
            add_to_album(album, files).await?;
        }
        CliSubCommands::Album(AlbumCommand {
            command: AlbumSubCommands::Upload(UploadFiles { album, files }),
        }) => {
            let urls = upload_files(files).await?;

            add_to_album(album, urls).await?;
        }
        CliSubCommands::Album(AlbumCommand {
            command: AlbumSubCommands::List(AlbumList { album: Some(album) }),
        }) => {
            let album = {
                if album.contains("catbox.moe") {
                    Album::new(Url::parse(&album)?)
                } else {
                    Album::new(Url::parse(&format!("https://catbox.moe/c/{album}"))?)
                }
            };

            let files = album.fetch_files().await?.urls;

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&files)?);
            } else {
                for (i, x) in files.into_iter().rev().enumerate() {
                    println!("File {}: {x}", i + 1);
                }
            }
        }
        CliSubCommands::Album(AlbumCommand {
            command: AlbumSubCommands::List(AlbumList { album: None }),
        }) => {
            let user = USER_INSTANCE.get().await?;

            let albums = user.fetch_albums().await?;

            if cli.json {
                let albums = albums.into_iter().map(|x| x.url).collect::<Vec<_>>();
                println!("{}", serde_json::to_string_pretty(&albums)?);
            } else {
                for (i, x) in user.fetch_albums().await?.into_iter().rev().enumerate() {
                    println!("Album {}: {}", i + 1, x.url);
                }
            }
        }
        CliSubCommands::Config(ConfigCommand {
            command: ConfigSubCommands::Save(SaveConfig { username, password }),
        }) => {
            get_username_entry()?.set_password(&username)?;
            get_password_entry()?.set_password(&password)?;
        }
        CliSubCommands::Config(ConfigCommand {
            command: ConfigSubCommands::Delete(DeleteConfig {}),
        }) => {
            get_username_entry()?.delete_credential()?;
            get_password_entry()?.delete_credential()?;
        }
    }

    Ok(())
}
