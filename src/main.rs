use std::{fs::File, path::PathBuf, sync::Arc};

use anyhow::anyhow;
use clap::{Parser, Subcommand};
use git2::Repository;
use log::{debug, error, info, trace, warn};
use notify::{
    EventKind, RecommendedWatcher, RecursiveMode, Watcher,
    event::{CreateKind, ModifyKind, RenameMode},
};
use owo_colors::OwoColorize;
use regex::Regex;
use zip::ZipArchive;

/// Automatically converts osu! exports into git commits
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Exports directory
    #[arg(short, long)]
    exports: Option<PathBuf>,

    /// Repositories directory
    #[arg(short, long)]
    repositories: Option<PathBuf>,

    /// Keep and commit latest .osz in the root of the repository
    /// Please note that this will at least double the size of the repository
    #[arg(short, long, action)]
    keep_latest_osz: bool,

    #[clap(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Manually import .osz file
    Import {
        /// .osz file to import
        #[arg()]
        file: PathBuf,

        /// Override target repository name
        #[arg(long)]
        use_repository: Option<String>,
    },
}

#[derive(Debug)]
struct Config {
    exports: PathBuf,
    repos: PathBuf,
    keep_latest_osz: bool,
}

impl Config {
    pub fn parse(args: &Args) -> anyhow::Result<Self> {
        let exports = match &args.exports {
            Some(p) => p.clone(),
            None => std::env::current_dir().expect("unable to get the current working directory"),
        };
        let repos = match &args.repositories {
            Some(p) => p.clone(),
            None => std::env::current_dir().expect("unable to get the current working directory"),
        };

        match std::fs::exists(&exports) {
            Ok(true) => {}
            Ok(false) => anyhow::bail!("Exports directory doesn't exist!"),
            Err(err) => anyhow::bail!("Failed to check exports directory: {}", err),
        };
        match std::fs::exists(&repos) {
            Ok(true) => {}
            Ok(false) => anyhow::bail!("Repositories directory doesn't exist!"),
            Err(err) => anyhow::bail!("Failed to check repositories directory: {}", err),
        };

        Ok(Self { exports, repos, keep_latest_osz: args.keep_latest_osz })
    }
}

fn main() -> anyhow::Result<()> {
    // if let Err(_) = std::env::var("RUST_LOG") {
    //     std::env::set_var("RUST_LOG", "info");
    // }
    pretty_env_logger::init();

    let args = Args::parse();
    let config = Arc::new(Config::parse(&args)?);

    // TODO: export command

    if let Some(command) = args.command {
        return command.run(config.clone());
    }

    watcher(config.clone())
}

fn watcher(config: Arc<Config>) -> anyhow::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = RecommendedWatcher::new(tx, notify::Config::default())?;
    watcher.watch(&config.exports, RecursiveMode::NonRecursive)?;

    let is_osz_path = |x: &PathBuf| {
        x.is_file() && x.extension().map(|x| x == "osz").unwrap_or(false) && x.file_name().is_some()
    };

    info!(
        "{} is now monitoring {}!",
        "gitosu".purple(),
        config.exports.to_string_lossy().purple()
    );

    for v in rx {
        match v {
            Ok(event) => {
                trace!("{:#?}", event);
                match event.kind {
                    EventKind::Create(CreateKind::File) => {
                        for path in event.paths.into_iter().filter(is_osz_path) {
                            match import_file(&path, config.clone(), None) {
                                Ok(_) => info!("Import completed! Don't forget to push!"),
                                Err(err) => error!("[{}] Import failed! {}", "x".red(), err),
                            };
                        }
                    }
                    EventKind::Modify(ModifyKind::Name(mode)) => {
                        let new_path = match mode {
                            // notify emits duplicate events (Both and From/To)
                            // RenameMode::Both => Some(event.paths[1].clone()),
                            RenameMode::To => Some(event.paths[0].clone()),
                            _ => None,
                        };
                        if let Some(path) = new_path {
                            if is_osz_path(&path) {
                                match import_file(&path, config.clone(), None) {
                                    Ok(_) => info!("Import completed! Don't forget to push!"),
                                    Err(err) => error!("[{}] Import failed! {}", "x".red(), err),
                                };
                            }
                        }
                    }
                    _ => {}
                }
            }
            Err(err) => {
                error!("Error while watching exports: {}", err);
            }
        }
    }

    Ok(())
}

fn import_file(path: &PathBuf, config: Arc<Config>, override_repo: Option<String>) -> anyhow::Result<()> {
    info!(
        "[{}] Importing {}...",
        "+".green(),
        path.file_name().unwrap().to_string_lossy().green()
    );

    let mut name: Option<String> = None;

    // Default naming
    let duplicate_regex = Regex::new(r"(.+? \(.+?\))( \((\d+)\))?\.osz").unwrap();
    for caps in duplicate_regex.captures_iter(&path.file_name().unwrap().to_string_lossy()) {
        if let Some(n) = caps.get(1) {
            name = Some(n.as_str().to_string());
            break;
        }
        // Map and author
        // info!("{:?}", caps.get(1));
        // Duplicate number
        // info!("{:?}", caps.get(3));
    }

    let name = match name {
        Some(n) => n,

        // Import was ran from `gitosu commit`
        None if override_repo.is_some() => override_repo.unwrap(),

        // File doesn't match the default naming scheme,
        // just use the file name without .osz
        None => {
            let file_name = path.file_name().unwrap().to_string_lossy().to_string();
            file_name[..(file_name.len() - 4)].to_string()
        }
    };

    info!("[{}] Using map repository {}", "i".cyan(), name.cyan());

    let repo_path = config.repos.join(&name);
    let repo_exists = match std::fs::exists(&repo_path) {
        Ok(v) => v,
        Err(err) => anyhow::bail!("Failed to check if repository exists: {}", err),
    };

    let repo = if repo_exists {
        match Repository::open(&repo_path) {
            Ok(repo) => repo,
            Err(err) => anyhow::bail!("Failed to open repository: {}", err),
        }
    } else {
        info!(
            "[{}] Initializing map repository at {}",
            "i".cyan(),
            repo_path.to_string_lossy().cyan()
        );
        match Repository::init(&repo_path) {
            Ok(repo) => repo,
            Err(err) => anyhow::bail!("Failed to init repository: {}", err),
        }
    };
    if !repo_exists {
        // Initialize basic repository
        std::fs::write(
            repo_path.join("README.md"),
            include_str!("defaultreadme.md").replace("{map_name}", &name),
        )
        .map_err(|x| anyhow!("Failed to write README.md: {}", x))?;
        std::fs::create_dir(repo_path.join("map"))
            .map_err(|x| anyhow!("Failed to create map directory: {}", x))?;
        git_add_all(&repo);
        git_initial_commit(&repo);
    }

    let file = File::open(path).map_err(|x| anyhow!("Failed to open .osz: {}", x))?;
    let mut zip = ZipArchive::new(file)
        .map_err(|x| anyhow!("Failed to open .osz as a zip archive: {}", x))?;
    if zip.len() == 0 {
        anyhow::bail!("Exported archive is empty!!!");
    }

    let map_path = repo_path.join("map");
    // Removing everything in the map directory
    // (the reason why you shouldn't touch it)
    if let Ok(true) = std::fs::exists(&map_path) {
        std::fs::remove_dir_all(&map_path)
            .map_err(|x| anyhow!("Failed to clear the map directory: {}", x))?;
    }

    // Copy latest files into the map directory
    info!("[{}] Importing files...", "i".cyan());
    for i in 0..zip.len() {
        let mut zip_file = zip.by_index(i)?;
        let zip_path = match zip_file.enclosed_name() {
            Some(p) => p,
            None => {
                warn!("[{}] Map archive contains forbidden files!", "!".yellow());
                continue;
            }
        };
        let target_path = map_path.join(&zip_path);
        debug!(
            "copying {} into {}",
            zip_path.to_string_lossy(),
            target_path.to_string_lossy()
        );
        let parent = target_path.parent().ok_or(anyhow!("Incorrect file path"))?;
        std::fs::create_dir_all(parent)
            .map_err(|x| anyhow!("Failed to make parent directories for file: {}", x))?;
        let mut file = File::create(&target_path)
            .map_err(|x| anyhow!("Failed to open target file for writing: {}", x))?;
        std::io::copy(&mut zip_file, &mut file)
            .map_err(|x| anyhow!("Failed to write file: {}", x))?;
    }

    if config.keep_latest_osz {
        std::fs::copy(path, repo_path.join(name + ".osz"))
            .map_err(|x| anyhow!("Failed to copy the latest .osz: {}", x))?;
    }

    info!("[{}] Commiting changes...", "i".cyan());
    git_add_all(&repo);
    git_commit(&repo);

    Ok(())
}

impl Commands {
    pub fn run(self, config: Arc<Config>) -> anyhow::Result<()> {
        match self {
            Self::Import { file, use_repository } => {
                match std::fs::exists(&file) {
                    Ok(true) => {}
                    Ok(false) => anyhow::bail!("File not found!"),
                    Err(err) => anyhow::bail!("Failed to check if file exists: {}", err),
                };
                match import_file(&file, config.clone(), use_repository) {
                    Ok(_) => info!("Import completed! Don't forget to push!"),
                    Err(err) => error!("[{}] Import failed! {}", "x".red(), err),
                };
            }
        }
        Ok(())
    }
}

// https://github.com/rust-lang/git2-rs/issues/561
fn git_add_all(repo: &Repository) {
    let mut index = repo.index().unwrap();
    index
        .add_all(&["."], git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
}

fn git_commit(repo: &Repository) {
    let mut index = repo.index().unwrap();
    let oid = index.write_tree().unwrap();
    let signature = repo.signature().unwrap();
    let parent_commit = repo.head().unwrap().peel_to_commit().unwrap();
    let tree = repo.find_tree(oid).unwrap();
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        "Map update",
        &tree,
        &[&parent_commit],
    )
    .unwrap();
}

fn git_initial_commit(repo: &git2::Repository) {
    let signature = repo.signature().unwrap();
    let oid = repo.index().unwrap().write_tree().unwrap();
    let tree = repo.find_tree(oid).unwrap();
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        "New osu! map",
        &tree,
        &[],
    )
    .unwrap();
}
