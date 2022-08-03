//! Contains utilities for copying, deleting, sorting, hashing files.

use std::fs::OpenOptions;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::marker::Sync;
use std::path::{Path, PathBuf};
use std::{fs, io};

use blake2::{Blake2b, Digest};
use hashbrown::HashSet;
use log::{error, info};
use rayon::prelude::*;
use seahash;

use crate::lumins::parse::Flag;
use crate::progress::PROGRESS_BAR;

/// Interface for all file structs to perform common operations
///
/// Ensures that all files (file, dir, symlink) have
/// a way of obtaining their path, copying, and deleting
pub trait FileOps {
    fn path(&self) -> &Path;
    fn as_path_buf(&self) -> PathBuf;
    fn remove(&self, path: &Path);
    fn copy(&self, src: &Path, dest: &Path);
}

/// A struct that represents a single file
#[derive(Hash, Eq, PartialEq, Debug, Clone)]
pub struct File {
    path: PathBuf,
    size: u64,
}

impl FileOps for File {
    fn path(&self) -> &Path {
        &self.path
    }
    fn as_path_buf(&self) -> PathBuf {
        self.path.clone()
    }
    fn remove(&self, path: &Path) {
        match fs::remove_file(&path) {
            Ok(_) => info!("Deleting file {:?}", path),
            Err(e) => error!("Error -- Deleting file {:?}: {}", path, e),
        }
    }
    fn copy(&self, src: &Path, dest: &Path) {
        match fs::copy(&src, &dest) {
            Ok(_) => info!("Copying file {:?} -> {:?}", src, dest),
            Err(e) => error!("Error -- Copying file {:?}: {}", src, e),
        }
    }
}

impl File {
    pub fn from(path: &str, size: u64) -> Self {
        File {
            path: PathBuf::from(path),
            size,
        }
    }

    #[allow(unused)]
    #[allow(clippy::unused_io_amount)]
    fn diff_copy(src: &Path, dest: &Path) -> Result<(), io::Error> {
        if !Path::new(&dest).exists() {
            fs::copy(&src, &dest)?;
        }

        const CHUNK_SIZE: usize = 10000;

        let src_file = fs::File::open(&src)?;
        let mut src_reader = BufReader::with_capacity(CHUNK_SIZE, &src_file);
        let dest_file = OpenOptions::new()
            .write(true)
            .read(true)
            .create(true)
            .open(&dest)?;
        dest_file.set_len(src_file.metadata()?.len())?;
        let mut dest_reader = BufReader::with_capacity(CHUNK_SIZE, &dest_file);
        let mut dest_writer = BufWriter::with_capacity(CHUNK_SIZE, &dest_file);

        loop {
            let mut src_buffer = [0; CHUNK_SIZE];
            let mut dest_buffer = [0; CHUNK_SIZE];

            if src_reader.read(&mut src_buffer)? == 0 {
                break;
            }
            dest_reader.read(&mut dest_buffer)?;

            if seahash::hash(&src_buffer) != seahash::hash(&dest_buffer) {
                dest_writer.write(&src_buffer)?;
            } else {
                dest_writer.seek(SeekFrom::Current(CHUNK_SIZE as i64));
            }
        }

        Ok(())
    }
}

/// A struct that represents a single directory
#[derive(Hash, Eq, PartialEq, Debug, Clone)]
pub struct Dir {
    path: PathBuf,
}

impl FileOps for Dir {
    fn path(&self) -> &Path {
        &self.path
    }
    fn as_path_buf(&self) -> PathBuf {
        self.path.clone()
    }
    fn remove(&self, path: &Path) {
        match fs::remove_dir(&path) {
            Ok(_) => info!("Deleting dir {:?}", path),
            Err(e) => error!("Error -- Deleting dir {:?}: {}", path, e),
        }
    }
    fn copy(&self, _src: &Path, dest: &Path) {
        match fs::create_dir_all(&dest) {
            Ok(_) => info!("Creating dir {:?}", dest),
            Err(e) => error!("Error -- Creating dir {:?}: {}", dest, e),
        }
    }
}

impl Dir {
    pub fn from(dir: &str) -> Self {
        Dir {
            path: PathBuf::from(dir),
        }
    }
}

/// A struct that represents a single symbolic link
#[derive(Hash, Eq, PartialEq, Debug, Clone)]
pub struct Symlink {
    path: PathBuf,
    target: PathBuf,
}

impl FileOps for Symlink {
    fn path(&self) -> &Path {
        &self.path
    }
    fn as_path_buf(&self) -> PathBuf {
        self.path.clone()
    }
    fn remove(&self, path: &Path) {
        match fs::remove_file(&path) {
            Ok(_) => info!("Deleting symlink {:?}", path),
            Err(e) => error!("Error -- Deleting symlink {:?}: {}", path, e),
        }
    }
    #[cfg(target_family = "unix")]
    fn copy(&self, _src: &Path, dest: &Path) {
        use std::os::unix::fs;

        match fs::symlink(&self.target, &dest) {
            Ok(_) => info!("Creating symlink {:?} -> {:?}", dest, self.target),
            Err(e) => error!("Error -- Creating symlink {:?}: {}", dest, e),
        }
    }
    #[cfg(target_family = "windows")]
    fn copy(&self, _src: &Path, dest: &Path) {
        use std::os::windows::fs;
        if self.target.is_file() {
            match fs::symlink_file(&self.target, &dest) {
                Ok(_) => info!("Creating symlink file {:?} -> {:?}", dest, self.target),
                Err(e) => error!("Error -- Creating symlink file{:?}: {}", dest, e),
            }
        }
        if self.target.is_dir() {
            match fs::symlink_dir(&self.target, &dest) {
                Ok(_) => info!("Creating symlink dir {:?} -> {:?}", dest, self.target),
                Err(e) => error!("Error -- Creating symlink dir {:?}: {}", dest, e),
            }
        }
    }
}

impl Symlink {
    pub fn from(path: &str, target: &str) -> Self {
        Symlink {
            path: PathBuf::from(path),
            target: PathBuf::from(target),
        }
    }
}

/// A struct that represents sets of different types of files
#[derive(Eq, PartialEq, Debug, Clone)]
pub struct FileSets {
    files: HashSet<File>,
    dirs: HashSet<Dir>,
    symlinks: HashSet<Symlink>,
}

impl FileSets {
    /// Initializes FileSets with the given sets
    ///
    /// # Arguments
    /// * `files`: a set of files
    /// * `dirs`: a set of dirs
    /// * `symlinks`: a set of symlinks
    ///
    /// # Returns
    /// A newly created FileSets struct
    pub fn with(files: HashSet<File>, dirs: HashSet<Dir>, symlinks: HashSet<Symlink>) -> Self {
        FileSets {
            files,
            dirs,
            symlinks,
        }
    }
    /// Gets the set of files
    ///
    /// # Returns
    /// The FileSets set of files
    pub fn files(&self) -> &HashSet<File> {
        &self.files
    }
    /// Gets the set of dirs
    ///
    /// # Returns
    /// The FileSets set of dirs
    pub fn dirs(&self) -> &HashSet<Dir> {
        &self.dirs
    }
    /// Gets the set of symlinks
    ///
    /// # Returns
    /// The FileSets set of symlinks
    pub fn symlinks(&self) -> &HashSet<Symlink> {
        &self.symlinks
    }
}

/// Compares all files in `files_to_compare` in `src` with all files in `files_to_compare` in `dest`
/// and copies them over if they are different, in parallel
///
/// # Arguments
/// * `files_to_compare`: files to compare
/// * `src`: base directory of the files to copy from, such that for all `file` in
/// `files_to_compare`, `src + file.path()` is the absolute path of the source file
/// * `dest`: base directory of the files to copy to, such that for all `file` in
/// `files_to_compare`, `dest + file.path()` is the absolute path of the destination file
/// * `flags`: set for Flag's
pub fn compare_and_copy_files<'a, T, S>(files_to_compare: T, src: &str, dest: &str, flags: Flag)
where
    T: ParallelIterator<Item = &'a S>,
    S: FileOps + Sync + 'a,
{
    files_to_compare.for_each(|file| {
        compare_and_copy_file(file, src, dest, flags);
        PROGRESS_BAR.inc(2);
    });
}

/// Compares the given file and copies the src file over if it differs from the dest file
///
/// # Arguments
/// * `file_to_compare`: file to compare
/// * `src`: base directory of the file to copy from, such that `src + file.path()`
/// is the absolute path of the source file
/// * `dest`: base directory of the files to copy to, such that `dest + file.path()`
/// is the absolute path of the destination file
/// * `flags`: set for Flag's
fn compare_and_copy_file<S>(file_to_compare: &S, src: &str, dest: &str, flags: Flag)
where
    S: FileOps,
{
    if flags.contains(Flag::SECURE) {
        let src_file_hash_secure = 
            hash_file_secure(file_to_compare, src);

        if src_file_hash_secure.is_none() {
            copy_file(file_to_compare, src, dest);
            return;
        }

        let dest_file_hash_secure =
            hash_file_secure(file_to_compare, dest);

        if src_file_hash_secure != dest_file_hash_secure {
            copy_file(file_to_compare, src, dest);
        }
    } else {
        let src_file_hash = hash_file(file_to_compare, src);

        if src_file_hash.is_none() {
            copy_file(file_to_compare, src, dest);
            return;
        }

        let dest_file_hash = hash_file(file_to_compare, dest);

        if src_file_hash != dest_file_hash {
            copy_file(file_to_compare, src, dest);
        }
    }
}

/// Copies all given files from `src` to `dest` in parallel
///
/// # Arguments
/// * `files_to_copy`: files to copy
/// * `src`: base directory of the files to copy from, such that for all `file` in
/// `files_to_copy`, `src + file.path()` is the absolute path of the source file
/// * `dest`: base directory of the files to copy to, such that for all `file` in
/// `files_to_copy`, `dest + file.path()` is the absolute path of the destination file
pub fn copy_files<'a, T, S>(files_to_copy: T, src: &str, dest: &str)
where
    T: ParallelIterator<Item = &'a S>,
    S: FileOps + Sync + 'a,
{
    files_to_copy.for_each(|file| {
        copy_file(file, src, dest);
        PROGRESS_BAR.inc(1);
    });
}

/// Copies a single file from `src` to `dest`
///
/// # Arguments
/// * `files_to_copy`: file to copy
/// * `src`: base directory of the files to copy from, such that `src + file_to_copy.path()`
/// is the absolute path of the source file
/// * `dest`: base directory of the files to copy to, such that `dest + file.path()`
/// is the absolute path of the destination file
fn copy_file<S>(file_to_copy: &S, src: &str, dest: &str)
where
    S: FileOps,
{
    let src_file = Path::new(src).join(file_to_copy.path());
    let dest_file = PathBuf::from(dest).join(file_to_copy.path());

    file_to_copy.copy(&src_file, &dest_file);
}

/// Deletes all given files in parallel
///
/// There is no guarantee that this function will delete the files in the given order
///
/// # Arguments
/// `files_to_delete`: files to delete
/// * `location`: base directory of the files to delete, such that for all `file` in
/// `files_to_delete`, `location + file.path()` is the absolute path of the file
pub fn delete_files<'a, T, S>(files_to_delete: T, location: &str)
where
    T: ParallelIterator<Item = &'a S>,
    S: FileOps + Sync + 'a,
{
    files_to_delete.for_each(|file| {
        // let path = [&Path::from(&location), file.path()].iter().collect();
        let path = PathBuf::from(location).join(file.path());
        file.remove(&path);
        PROGRESS_BAR.inc(1);
    });
}

/// Deletes all given files sequentially
///
/// This function ensures that the files are deleted in the exact order given
///
/// # Arguments
/// * `files_to_delete`: files to delete, or sorted empty directories
/// * `location`: base directory of the files to delete, such that for all `file` in
/// `files_to_delete`, `location + file.path()` is the absolute path of the file
pub fn delete_files_sequential<'a, T, S>(files_to_delete: T, location: &str)
where
    T: IntoIterator<Item = &'a S>,
    S: FileOps + 'a,
{
    for file in files_to_delete {
        // let path = [&Path::from(&location), file.path()].iter().collect();
        let path = PathBuf::from(location).join(file.path());
        file.remove(&path);
        PROGRESS_BAR.inc(1);
    }
}

/// Sorts (unstable) file paths in descending order by number of components, in parallel
///
/// # Arguments
/// `files_to_sort`: files to sort
///
/// # Returns
/// A vector of file paths in descending order by number of components
///
/// # Examples
/// ["a", "a/b", "a/b/c"] becomes ["a/b/c", "a/b", "a"]
/// ["/usr", "/", "/usr/bin", "/etc"] becomes ["/usr/bin", "/usr", "/etc", "/"]
pub fn sort_files<'a, T, S>(files_to_sort: T) -> Vec<&'a S>
where
    T: ParallelIterator<Item = &'a S>,
    S: FileOps + Sync + 'a,
{
    let mut files_to_sort = Vec::from_par_iter(files_to_sort);
    files_to_sort.par_sort_unstable_by(|a, b| {
        b.path()
            .components()
            .count()
            .cmp(&a.path().components().count())
    });
    files_to_sort
}

/// Generates a hash of the given file, using the Seahash non-cryptographic hash function
///
/// # Arguments
/// * `file_to_hash`: file object to hash
/// * `location`: base directory of the file to hash, such that
/// `location + file_to_hash.path()` is the absolute path of the file
///
/// # Returns
/// * Some: The hash of the given file
/// * Err: If the given file cannot be hashed
pub fn hash_file<S>(file_to_hash: &S, location: &str) -> Option<u64>
where
    S: FileOps,
{
    let file = PathBuf::from(location).join(file_to_hash.path());
    match fs::read(file) {
        Ok(contents) => Some(seahash::hash(&contents)),
        Err(_) => None,
    }
}

/// Generates a hash of the given file, using the BLAKE2b cryptographic hash function
///
/// # Arguments
/// * `file_to_hash`: file object to hash
/// * `location`: base directory of the file to hash, such that
/// `location + file_to_hash.path()` is the absolute path of the file
///
/// # Returns
/// * Some: The hash of the given file
/// * Err: If the given file cannot be hashed
pub fn hash_file_secure<S>(file_to_hash: &S, location: &str) -> Option<Vec<u8>>
where
    S: FileOps,
{
    let file = PathBuf::from(location).join(file_to_hash.path());
    match &mut fs::File::open(&file) {
        Ok(file) => {
            let mut hasher = Blake2b::new();

            match io::copy(file, &mut hasher) {
                Ok(_) => Some(hasher.finalize().to_vec()),
                Err(e) => {
                    error!("Error -- Hashing: {:?}: {}", file_to_hash.path(), e);
                    None
                }
            }
        }
        Err(e) => {
            error!("Error -- Opening File: {:?}: {}", file_to_hash.path(), e);
            None
        }
    }
}

/// Recursively traverses a directory and all its subdirectories and returns
/// a FileSets that contains all files and all directories
///
/// # Arguments
/// * `src`: directory to traverse
///
/// # Returns
/// * Ok: A `FileSets` containing a set of files a set of directories
/// * Error: If `src` is an invalid directory
pub fn get_all_files(src: &str) -> Result<FileSets, io::Error> {
    get_all_files_helper(&PathBuf::from(src), src)
}

/// Recursive helper for `get_all_files`
///
/// # Arguments
/// * `src`: directory to traverse
/// * `base`: directory to traverse, used for recursive calls
///
/// # Returns
/// * Ok: A `FileSets` containing a set of files a set of directories
/// * Error: If `src` is an invalid directory
fn get_all_files_helper(src: &Path, base: &str) -> Result<FileSets, io::Error> {
    let dir = src.read_dir()?;

    let mut files = HashSet::new();
    let mut dirs = HashSet::new();
    let mut symlinks = HashSet::new();

    for file in dir {
        if file.is_err() {
            error!("{}", file.err().unwrap());
            continue;
        }

        let file = file.unwrap();
        let metadata = file.metadata();

        if metadata.is_err() {
            error!(
                "Error -- Reading metadata of {:?} {}",
                file.path(),
                metadata.err().unwrap()
            );
            continue;
        }

        let metadata = metadata.unwrap();

        let path = file.path();
        // This is safe to unwrap, since `get_all_files` always calls this helper
        // with `base` equal to `src`
        let relative_path = path.strip_prefix(base).unwrap();

        if metadata.is_dir() {
            dirs.insert(Dir {
                path: relative_path.to_path_buf(),
            });

            // Recursively call `get_all_files_helper` on the subdirectory
            match get_all_files_helper(&file.path(), base) {
                Ok(file_sets) => {
                    // Add subdirectory subdirectories and files to sets
                    files.extend(file_sets.files);
                    dirs.extend(file_sets.dirs);
                    symlinks.extend(file_sets.symlinks);
                }
                Err(e) => {
                    error!("Error - Retrieving files: {}", e);
                    continue;
                }
            }
        } else if metadata.is_file() {
            files.insert(File {
                path: relative_path.to_path_buf(),
                size: metadata.len(),
            });
        } else {
            // If not a file nor dir, must be a symlink
            match fs::read_link(&path) {
                Ok(target) => {
                    symlinks.insert(Symlink {
                        path: relative_path.to_path_buf(),
                        target,
                    });
                }
                Err(e) => {
                    error!("Error - Reading symlink: {}", e);
                    continue;
                }
            }
        }
    }

    Ok(FileSets::with(files, dirs, symlinks))
}

///////////////////////////////////////////////////////////////////////////////////////////////////
// Tests
///////////////////////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod test_file_ops {
    use super::*;

    #[test]
    fn create_dir() {
        assert_eq!(
            Dir::from("."),
            Dir {
                path: PathBuf::from("."),
            }
        )
    }

    #[test]
    fn create_file() {
        assert_eq!(
            File::from(".", 10),
            File {
                path: PathBuf::from("."),
                size: 10,
            }
        )
    }

    #[test]
    fn create_symlink() {
        assert_eq!(
            Symlink::from(".", "file"),
            Symlink {
                path: PathBuf::from("."),
                target: PathBuf::from("file"),
            }
        )
    }
}

#[cfg(test)]
mod test_get_all_files {
    use super::*;
    use std::process::Command;

    #[test]
    fn invalid_dir() {
        assert_eq!(get_all_files("/?").is_err(), true);
    }

    #[cfg(target_family = "unix")]
    #[test]
    fn dir_insufficient_permissions() {
        assert_eq!(get_all_files("/root").is_err(), true);
    }

    #[test]
    fn empty_dir() {
        const TEST_DIR: &str = "test_get_all_files_empty_dir";

        fs::create_dir(TEST_DIR).unwrap();

        let file_sets = get_all_files(TEST_DIR).unwrap();

        assert_eq!(file_sets.files(), &HashSet::new());
        assert_eq!(file_sets.dirs(), &HashSet::new());

        fs::remove_dir(TEST_DIR).unwrap();
    }

    #[test]
    fn single_dir() {
        const TEST_DIR: &str = "test_get_all_files_single_dir";
        const TEST_SUB_DIR: &str = "test";

        fs::create_dir_all([TEST_DIR, TEST_SUB_DIR].join("/")).unwrap();

        let file_sets = get_all_files(&TEST_DIR).unwrap();
        let mut dir_set = HashSet::new();
        dir_set.insert(Dir {
            path: PathBuf::from(&TEST_SUB_DIR),
        });

        assert_eq!(file_sets.files(), &HashSet::new());
        assert_eq!(file_sets.dirs(), &dir_set);

        fs::remove_dir_all(&TEST_DIR).unwrap();
    }

    #[test]
    fn single_file() {
        const TEST_DIR: &str = "test_get_all_files_single_file";
        const TEST_FILE: &str = "file.txt";

        fs::create_dir_all(TEST_DIR).unwrap();

        fs::File::create([TEST_DIR, TEST_FILE].join("/")).unwrap();
        fs::write([TEST_DIR, TEST_FILE].join("/"), b"1234").unwrap();

        let file_sets = get_all_files(TEST_DIR).unwrap();
        let mut file_set = HashSet::new();
        file_set.insert(File {
            path: PathBuf::from(TEST_FILE),
            size: 4,
        });

        assert_eq!(file_sets.files(), &file_set);
        assert_eq!(file_sets.dirs(), &HashSet::new());

        fs::remove_dir_all(TEST_DIR).unwrap();
    }

    #[cfg(target_family = "unix")]
    #[test]
    fn single_symlink() {
        use std::os::unix::fs::symlink;
        const TEST_DIR: &str = "test_get_all_files_single_symlink";
        const TEST_LINK: &str = "test_get_all_files_single_symlink/file";
        const TEST_FILE: &str = "test_get_all_files_single_symlink/test.txt";

        fs::create_dir_all(TEST_DIR).unwrap();
        symlink(TEST_FILE, TEST_LINK).unwrap();

        let mut symlink_set = HashSet::new();
        symlink_set.insert(Symlink {
            path: PathBuf::from("file"),
            target: PathBuf::from(TEST_FILE),
        });

        let file_sets = get_all_files(TEST_DIR).unwrap();

        assert_eq!(
            file_sets,
            FileSets {
                files: HashSet::new(),
                dirs: HashSet::new(),
                symlinks: symlink_set,
            }
        );

        fs::remove_dir_all(TEST_DIR).unwrap();
    }

    #[test]
    fn multi_level() {
        const TEST_DIR: &str = "test_get_all_files_multi_level";
        const SUB_DIRS: [&str; 2] = ["dir1", "dir1/dir2"];
        const TEST_FILES: [&str; 3] = ["file.txt", "dir1/file.txt", "dir1/dir2/file2.txt"];
        const TEST_DATA: [&[u8]; 3] = [b"1", b"", b"1234567890"];

        fs::create_dir_all([TEST_DIR, SUB_DIRS[1]].join("/")).unwrap();

        for i in 0..TEST_FILES.len() {
            let path = [TEST_DIR, TEST_FILES[i]].join("/");
            fs::File::create(&path).unwrap();
            fs::write(&path, TEST_DATA[i]).unwrap();
        }

        let file_sets = get_all_files(TEST_DIR).unwrap();
        let mut file_set = HashSet::new();
        let mut dir_set = HashSet::new();

        for i in 0..TEST_FILES.len() {
            file_set.insert(File {
                path: PathBuf::from(TEST_FILES[i]),
                size: TEST_DATA[i].len() as u64,
            });
        }

        for i in 0..SUB_DIRS.len() {
            dir_set.insert(Dir {
                path: PathBuf::from(SUB_DIRS[i]),
            });
        }

        assert_eq!(file_sets.files(), &file_set);
        assert_eq!(file_sets.dirs(), &dir_set);

        fs::remove_dir_all(TEST_DIR).unwrap();
    }

    #[cfg(target_family = "unix")]
    #[test]
    fn multi_level_insufficient_permissions() {
        const TEST_DIR: &str = "test_get_all_files_multi_level_insufficient_permissions";
        const SUB_DIR: &str = "dir";
        const TEST_FILE: &str = "file.txt";

        let file_path = [TEST_DIR, TEST_FILE].join("/");
        let dir_path = [TEST_DIR, SUB_DIR].join("/");

        fs::create_dir_all(&dir_path).unwrap();
        fs::File::create(&file_path).unwrap();

        Command::new("chmod")
            .args(&["000", &file_path])
            .output()
            .unwrap();
        Command::new("chmod")
            .args(&["000", &dir_path])
            .output()
            .unwrap();

        let file_sets = get_all_files(TEST_DIR).unwrap();

        let mut file_set = HashSet::new();
        file_set.insert(File {
            path: PathBuf::from(&TEST_FILE),
            size: 0,
        });
        let mut dir_set = HashSet::new();
        dir_set.insert(Dir {
            path: PathBuf::from(&SUB_DIR),
        });

        assert_eq!(file_sets.files(), &file_set);
        assert_eq!(file_sets.dirs(), &dir_set);

        Command::new("chmod")
            .arg("777")
            .args(&["777", &dir_path])
            .output()
            .unwrap();
        fs::remove_dir_all(TEST_DIR).unwrap();
    }
}

#[cfg(test)]
mod test_sort_files {
    use super::*;

    #[test]
    fn no_dir() {
        let no_dir: HashSet<Dir> = HashSet::new();
        assert_eq!(sort_files(no_dir.par_iter()), Vec::<&Dir>::new());
    }

    #[test]
    fn single_dir() {
        let mut single_dir: HashSet<Dir> = HashSet::new();
        let dir = Dir {
            path: PathBuf::from("/"),
        };
        single_dir.insert(dir.clone());
        let expected: Vec<&Dir> = vec![&dir];

        assert_eq!(sort_files(single_dir.par_iter()), expected);
    }

    #[test]
    fn multi_dir_unique() {
        let mut multi_dir: HashSet<Dir> = HashSet::new();
        let dir1 = Dir {
            path: PathBuf::from("/"),
        };
        let dir2 = Dir {
            path: PathBuf::from("/a"),
        };
        let dir3 = Dir {
            path: PathBuf::from("/a/b"),
        };
        multi_dir.insert(dir1.clone());
        multi_dir.insert(dir2.clone());
        multi_dir.insert(dir3.clone());
        let expected: Vec<&Dir> = vec![&dir3, &dir2, &dir1];

        assert_eq!(sort_files(multi_dir.par_iter()), expected);
    }

    #[test]
    fn multi_dir() {
        let mut multi_dir: HashSet<Dir> = HashSet::new();
        let dir1 = Dir {
            path: PathBuf::from("/"),
        };
        let dir2 = Dir {
            path: PathBuf::from("/a/c"),
        };
        let dir3 = Dir {
            path: PathBuf::from("/a/b"),
        };
        multi_dir.insert(dir1.clone());
        multi_dir.insert(dir2.clone());
        multi_dir.insert(dir3.clone());
        let expected: Vec<&Dir> = vec![&dir2, &dir3, &dir1];

        assert_eq!(
            sort_files(multi_dir.par_iter()).get(2).unwrap(),
            &expected[2]
        );
    }
}

#[cfg(test)]
mod test_hash_file {
    use super::*;

    #[test]
    fn invalid_file() {
        assert_eq!(
            hash_file(
                &File {
                    path: PathBuf::from("test"),
                    size: 0,
                },
                "."
            ),
            None
        );
    }

    #[test]
    fn empty_file() {
        const TEST_FILE1: &str = "test_hash_file_empty_file1.txt";
        const TEST_FILE2: &str = "test_hash_file_empty_file2.txt";

        fs::File::create(TEST_FILE1).unwrap();
        fs::File::create(TEST_FILE2).unwrap();

        assert_eq!(
            hash_file(
                &File {
                    path: PathBuf::from(TEST_FILE1),
                    size: 0,
                },
                "."
            ),
            hash_file(
                &File {
                    path: PathBuf::from(TEST_FILE2),
                    size: 0,
                },
                "."
            )
        );
        assert_eq!(
            hash_file_secure(
                &File {
                    path: PathBuf::from(TEST_FILE1),
                    size: 0,
                },
                "."
            ),
            hash_file_secure(
                &File {
                    path: PathBuf::from(TEST_FILE2),
                    size: 0,
                },
                "."
            )
        );

        fs::remove_file(TEST_FILE1).unwrap();
        fs::remove_file(TEST_FILE2).unwrap();
    }

    #[test]
    fn equal_files() {
        const TEST_DIR: &str = "test_hash_file_equal_files";
        const TEST_FILE1: &str = "file1.txt";
        const TEST_FILE2: &str = "file2.txt";

        let path1 = [TEST_DIR, TEST_FILE1].join("/");
        let path2 = [TEST_DIR, TEST_FILE2].join("/");

        fs::create_dir_all(TEST_DIR).unwrap();
        fs::File::create(&path1).unwrap();
        fs::File::create(&path2).unwrap();
        fs::write(path1, b"1234567890").unwrap();
        fs::write(path2, b"1234567890").unwrap();

        assert_eq!(
            hash_file(
                &File {
                    path: PathBuf::from(TEST_FILE1),
                    size: 10,
                },
                "."
            ),
            hash_file(
                &File {
                    path: PathBuf::from(TEST_FILE2),
                    size: 10,
                },
                "."
            )
        );
        assert_eq!(
            hash_file_secure(
                &File {
                    path: PathBuf::from(TEST_FILE1),
                    size: 10,
                },
                "."
            ),
            hash_file_secure(
                &File {
                    path: PathBuf::from(TEST_FILE2),
                    size: 10,
                },
                "."
            )
        );

        fs::remove_dir_all(TEST_DIR).unwrap();
    }

    #[test]
    fn different_files() {
        assert_ne!(
            hash_file(
                &File {
                    path: PathBuf::from("lumins/file_ops.rs"),
                    size: 0,
                },
                "src"
            ),
            hash_file(
                &File {
                    path: PathBuf::from("main.rs"),
                    size: 0,
                },
                "src"
            )
        );
        assert_ne!(
            hash_file_secure(
                &File {
                    path: PathBuf::from("lumins/file_ops.rs"),
                    size: 0,
                },
                "src"
            ),
            hash_file_secure(
                &File {
                    path: PathBuf::from("main.rs"),
                    size: 0,
                },
                "src"
            )
        );
    }
}

#[cfg(test)]
mod test_delete_files {
    use super::*;

    #[test]
    fn delete_no_files() {
        const TEST_DIR: &str = "test_delete_files_delete_no_files";
        const TEST_FILES: [&str; 2] = ["file1.txt", "file2.txt"];

        fs::create_dir_all(TEST_DIR).unwrap();

        let files_to_delete: HashSet<File> = HashSet::new();
        let files_to_delete_sequential: Vec<&File> = Vec::new();
        let mut file_set = HashSet::new();

        for i in 0..TEST_FILES.len() {
            fs::File::create([TEST_DIR, TEST_FILES[i]].join("/")).unwrap();
            let file = File {
                path: PathBuf::from(TEST_FILES[i]),
                size: 0,
            };
            file_set.insert(file);
        }

        delete_files(files_to_delete.par_iter(), TEST_DIR);
        delete_files_sequential(files_to_delete_sequential.into_iter(), TEST_DIR);

        assert_eq!(
            get_all_files(TEST_DIR).unwrap(),
            FileSets {
                files: file_set,
                dirs: HashSet::new(),
                symlinks: HashSet::new(),
            }
        );

        fs::remove_dir_all(TEST_DIR).unwrap();
    }

    #[cfg(target_family = "unix")]
    #[test]
    fn delete_invalid_file_and_link() {
        use std::os::unix::fs::symlink;

        const TEST_DIR: &str = "test_delete_files_delete_invalid_file_and_link";
        const TEST_DIR_SEQ: &str = "test_delete_files_delete_invalid_file_and_link_seq";
        const TEST_FILES: [&str; 2] = ["file1.txt", "file2.txt"];

        fs::create_dir_all(TEST_DIR).unwrap();
        fs::create_dir_all(TEST_DIR_SEQ).unwrap();

        let mut files_to_delete: HashSet<File> = HashSet::new();
        let mut files_to_delete_sequential: Vec<&File> = Vec::new();
        let mut file_set = HashSet::new();

        fs::File::create([TEST_DIR, TEST_FILES[0]].join("/")).unwrap();
        fs::File::create([TEST_DIR_SEQ, TEST_FILES[0]].join("/")).unwrap();
        let file = File {
            path: PathBuf::from([TEST_FILES[0], "a"].join("/")),
            size: 0,
        };
        let expected_file = File {
            path: PathBuf::from(TEST_FILES[0]),
            size: 0,
        };
        file_set.insert(expected_file);
        files_to_delete.insert(file.clone());
        files_to_delete_sequential.push(&file);

        let mut links_to_delete: HashSet<Symlink> = HashSet::new();
        let mut links_to_delete_sequential: Vec<&Symlink> = Vec::new();
        let mut link_set = HashSet::new();

        symlink(TEST_FILES[1], [TEST_DIR, "file"].join("/")).unwrap();
        symlink(TEST_FILES[1], [TEST_DIR_SEQ, "file"].join("/")).unwrap();
        let link = Symlink {
            path: PathBuf::from("filea"),
            target: PathBuf::from(TEST_FILES[1]),
        };
        let expected_link = Symlink {
            path: PathBuf::from("file"),
            target: PathBuf::from(TEST_FILES[1]),
        };
        link_set.insert(expected_link);
        links_to_delete.insert(link.clone());
        links_to_delete_sequential.push(&link);

        delete_files(files_to_delete.par_iter(), TEST_DIR);
        delete_files_sequential(files_to_delete_sequential.into_iter(), TEST_DIR_SEQ);
        delete_files(links_to_delete.par_iter(), TEST_DIR);
        delete_files_sequential(links_to_delete_sequential.into_iter(), TEST_DIR_SEQ);

        assert_eq!(
            get_all_files(TEST_DIR).unwrap(),
            FileSets {
                files: file_set.clone(),
                dirs: HashSet::new(),
                symlinks: link_set.clone(),
            }
        );
        assert_eq!(
            get_all_files(TEST_DIR_SEQ).unwrap(),
            FileSets {
                files: file_set,
                dirs: HashSet::new(),
                symlinks: link_set,
            }
        );

        fs::remove_dir_all(TEST_DIR).unwrap();
        fs::remove_dir_all(TEST_DIR_SEQ).unwrap();
    }

    #[cfg(target_family = "unix")]
    #[test]
    fn delete_file_and_link() {
        use std::os::unix::fs::symlink;

        const TEST_DIR: &str = "test_delete_files_delete_file_and_link";
        const TEST_DIR_SEQ: &str = "test_delete_files_delete_file_and_link_seq";
        const TEST_FILES: [&str; 2] = ["file1.txt", "file2.txt"];

        fs::create_dir_all(TEST_DIR).unwrap();
        fs::create_dir_all(TEST_DIR_SEQ).unwrap();

        let mut files_to_delete: HashSet<File> = HashSet::new();
        let mut files_to_delete_sequential: Vec<&File> = Vec::new();
        let mut file_set = HashSet::new();

        fs::File::create([TEST_DIR, TEST_FILES[0]].join("/")).unwrap();
        fs::File::create([TEST_DIR_SEQ, TEST_FILES[0]].join("/")).unwrap();
        let file = File {
            path: PathBuf::from(TEST_FILES[0]),
            size: 0,
        };
        file_set.insert(file.clone());
        files_to_delete.insert(file.clone());
        files_to_delete_sequential.push(&file);

        let mut links_to_delete: HashSet<Symlink> = HashSet::new();
        let mut links_to_delete_sequential: Vec<&Symlink> = Vec::new();
        let mut link_set = HashSet::new();

        symlink(TEST_FILES[1], [TEST_DIR, "file"].join("/")).unwrap();
        symlink(TEST_FILES[1], [TEST_DIR_SEQ, "file"].join("/")).unwrap();
        let link = Symlink {
            path: PathBuf::from("file"),
            target: PathBuf::from(TEST_FILES[1]),
        };
        link_set.insert(link.clone());
        links_to_delete.insert(link.clone());
        links_to_delete_sequential.push(&link);

        delete_files(files_to_delete.par_iter(), TEST_DIR);
        delete_files_sequential(files_to_delete_sequential.into_iter(), TEST_DIR_SEQ);
        delete_files(links_to_delete.par_iter(), TEST_DIR);
        delete_files_sequential(links_to_delete_sequential.into_iter(), TEST_DIR_SEQ);

        assert_eq!(
            get_all_files(TEST_DIR).unwrap(),
            FileSets {
                files: HashSet::new(),
                dirs: HashSet::new(),
                symlinks: HashSet::new(),
            }
        );
        assert_eq!(
            get_all_files(TEST_DIR_SEQ).unwrap(),
            FileSets {
                files: HashSet::new(),
                dirs: HashSet::new(),
                symlinks: HashSet::new(),
            }
        );

        fs::remove_dir_all(TEST_DIR).unwrap();
        fs::remove_dir_all(TEST_DIR_SEQ).unwrap();
    }

    #[test]
    fn delete_partial_dirs() {
        const TEST_DIR: &str = "test_delete_files_delete_partial_dirs";
        const TEST_DIR_SEQ: &str = "test_delete_files_delete_partial_dirs_seq";
        const TEST_SUB_DIRS: [&str; 3] = ["dir0", "dir1", "dir2"];

        fs::create_dir_all([TEST_DIR, TEST_SUB_DIRS[0], TEST_SUB_DIRS[1]].join("/")).unwrap();
        fs::create_dir_all([TEST_DIR_SEQ, TEST_SUB_DIRS[0], TEST_SUB_DIRS[1]].join("/")).unwrap();
        fs::create_dir_all([TEST_DIR, TEST_SUB_DIRS[2]].join("/")).unwrap();
        fs::create_dir_all([TEST_DIR_SEQ, TEST_SUB_DIRS[2]].join("/")).unwrap();

        let mut dirs_to_delete: HashSet<Dir> = HashSet::new();
        let mut dirs_to_delete_sequential: Vec<&Dir> = Vec::new();
        let mut file_set: HashSet<Dir> = HashSet::new();

        let dir0 = Dir {
            path: PathBuf::from(TEST_SUB_DIRS[0]),
        };
        let dir2 = Dir {
            path: PathBuf::from(TEST_SUB_DIRS[2]),
        };

        dirs_to_delete.insert(dir0.clone());
        dirs_to_delete.insert(dir2.clone());
        dirs_to_delete_sequential.push(&dir0);
        dirs_to_delete_sequential.push(&dir2);

        delete_files(dirs_to_delete.par_iter(), TEST_DIR);
        delete_files_sequential(dirs_to_delete_sequential.into_iter(), TEST_DIR_SEQ);

        file_set.insert(Dir {
            path: PathBuf::from(TEST_SUB_DIRS[0]),
        });
        file_set.insert(Dir {
            path: PathBuf::from([TEST_SUB_DIRS[0], TEST_SUB_DIRS[1]].join("/")),
        });

        assert_eq!(
            get_all_files(TEST_DIR).unwrap(),
            FileSets {
                files: HashSet::new(),
                dirs: file_set.clone(),
                symlinks: HashSet::new(),
            }
        );
        assert_eq!(
            get_all_files(TEST_DIR_SEQ).unwrap(),
            FileSets {
                files: HashSet::new(),
                dirs: file_set,
                symlinks: HashSet::new(),
            }
        );

        fs::remove_dir_all(TEST_DIR).unwrap();
        fs::remove_dir_all(TEST_DIR_SEQ).unwrap();
    }
}

#[cfg(test)]
mod test_copy_files {
    use super::*;
    use std::process::Command;

    #[test]
    fn no_files() {
        const TEST_DIR: &str = "test_copy_files_no_files";
        const TEST_DIR_OUT: &str = "test_copy_files_no_files_out";

        fs::create_dir_all(TEST_DIR).unwrap();
        fs::create_dir_all(TEST_DIR_OUT).unwrap();

        copy_files(HashSet::<File>::new().par_iter(), TEST_DIR, TEST_DIR_OUT);

        assert_eq!(
            get_all_files(TEST_DIR_OUT).unwrap(),
            FileSets {
                files: HashSet::new(),
                dirs: HashSet::new(),
                symlinks: HashSet::new(),
            }
        );

        fs::remove_dir_all(TEST_DIR).unwrap();
        fs::remove_dir_all(TEST_DIR_OUT).unwrap();
    }

    #[test]
    fn regular_files_dirs() {
        const TEST_DIR: &str = "src";
        const TEST_DIR_OUT: &str = "test_copy_files_regular_files_dirs_out";

        fs::create_dir_all(TEST_DIR_OUT).unwrap();

        copy_files(
            get_all_files(TEST_DIR).unwrap().dirs().par_iter(),
            TEST_DIR,
            TEST_DIR_OUT,
        );
        copy_files(
            get_all_files(TEST_DIR).unwrap().files().par_iter(),
            TEST_DIR,
            TEST_DIR_OUT,
        );

        assert_eq!(
            get_all_files(TEST_DIR_OUT).unwrap(),
            get_all_files(TEST_DIR).unwrap()
        );

        fs::remove_dir_all(TEST_DIR_OUT).unwrap();
    }

    #[test]
    #[cfg(target_family = "unix")]
    fn insufficient_output_permissions() {
        const TEST_DIR: &str = "src";
        const TEST_DIR_OUT: &str = "test_copy_files_insufficient_output_permissions_out";
        const SUB_DIR: &str = "lumins";

        fs::create_dir_all([TEST_DIR_OUT, SUB_DIR].join("/")).unwrap();
        fs::File::create([TEST_DIR_OUT, "main.rs"].join("/")).unwrap();
        fs::File::create([TEST_DIR_OUT, "cli.yml"].join("/")).unwrap();
        fs::File::create([TEST_DIR_OUT, "lib.rs"].join("/")).unwrap();
        Command::new("chmod")
            .arg("000")
            .arg([TEST_DIR_OUT, SUB_DIR].join("/"))
            .output()
            .unwrap();
        Command::new("chmod")
            .arg("000")
            .arg([TEST_DIR_OUT, "main.rs"].join("/"))
            .output()
            .unwrap();
        Command::new("chmod")
            .arg("000")
            .arg([TEST_DIR_OUT, "cli.yml"].join("/"))
            .output()
            .unwrap();
        Command::new("chmod")
            .arg("000")
            .arg([TEST_DIR_OUT, "lib.rs"].join("/"))
            .output()
            .unwrap();

        copy_files(
            get_all_files(TEST_DIR).unwrap().dirs().par_iter(),
            TEST_DIR,
            TEST_DIR_OUT,
        );
        copy_files(
            get_all_files(TEST_DIR).unwrap().files().par_iter(),
            TEST_DIR,
            TEST_DIR_OUT,
        );

        let mut files = HashSet::new();
        files.insert(File {
            path: PathBuf::from("main.rs"),
            size: 0,
        });
        files.insert(File {
            path: PathBuf::from("cli.yml"),
            size: 0,
        });
        files.insert(File {
            path: PathBuf::from("lib.rs"),
            size: 0,
        });
        let mut dirs = HashSet::new();
        dirs.insert(Dir {
            path: PathBuf::from("lumins"),
        });

        assert_eq!(
            get_all_files(TEST_DIR_OUT).unwrap(),
            FileSets {
                files: files.clone(),
                dirs: dirs.clone(),
                symlinks: HashSet::new(),
            }
        );

        Command::new("rm")
            .arg("-rf")
            .arg(TEST_DIR_OUT)
            .output()
            .unwrap();
    }

    #[test]
    #[cfg(target_family = "unix")]
    fn insufficient_input_permissions() {
        const TEST_DIR: &str = "test_copy_files_insufficient_input_permissions";
        const TEST_DIR_OUT: &str = "test_copy_files_insufficient_input_permissions_out";

        fs::create_dir_all(TEST_DIR).unwrap();
        fs::create_dir_all(TEST_DIR_OUT).unwrap();

        Command::new("cp")
            .args(&["-r", "src/lumins", TEST_DIR])
            .output()
            .unwrap();
        Command::new("cp")
            .args(&["src/main.rs", TEST_DIR])
            .output()
            .unwrap();
        Command::new("chmod")
            .arg("000")
            .arg([TEST_DIR, "lumins"].join("/"))
            .output()
            .unwrap();
        Command::new("chmod")
            .arg("000")
            .arg([TEST_DIR, "main.rs"].join("/"))
            .output()
            .unwrap();

        copy_files(
            get_all_files(TEST_DIR).unwrap().dirs().par_iter(),
            TEST_DIR,
            TEST_DIR_OUT,
        );
        copy_files(
            get_all_files(TEST_DIR).unwrap().files().par_iter(),
            TEST_DIR,
            TEST_DIR_OUT,
        );

        let files = HashSet::new();
        let mut dirs = HashSet::new();
        dirs.insert(Dir {
            path: PathBuf::from("lumins"),
        });

        assert_eq!(
            get_all_files(TEST_DIR_OUT).unwrap(),
            FileSets {
                files: files.clone(),
                dirs: dirs.clone(),
                symlinks: HashSet::new(),
            }
        );

        Command::new("chmod")
            .arg("777")
            .arg([TEST_DIR, "lumins"].join("/"))
            .output()
            .unwrap();
        Command::new("rm")
            .args(&["-rf", TEST_DIR])
            .output()
            .unwrap();
        Command::new("rm")
            .args(&["-rf", TEST_DIR_OUT])
            .output()
            .unwrap();
    }

    #[test]
    #[cfg(target_family = "unix")]
    fn copy_symlink() {
        use std::os::unix::fs::symlink;
        const TEST_DIR: &str = "test_copy_files_copy_symlink";
        const TEST_DIR_OUT: &str = "test_copy_files_copy_symlink_out_seq";

        fs::create_dir_all(TEST_DIR).unwrap();
        fs::create_dir_all(TEST_DIR_OUT).unwrap();
        symlink("src/main.rs", [TEST_DIR, "file"].join("/")).unwrap();

        copy_files(
            get_all_files(TEST_DIR).unwrap().symlinks().par_iter(),
            TEST_DIR,
            TEST_DIR_OUT,
        );

        let mut links_set = HashSet::new();
        links_set.insert(Symlink {
            path: PathBuf::from("file"),
            target: PathBuf::from("src/main.rs"),
        });

        assert_eq!(
            get_all_files(TEST_DIR_OUT).unwrap(),
            FileSets {
                files: HashSet::new(),
                dirs: HashSet::new(),
                symlinks: links_set.clone(),
            }
        );

        fs::remove_dir_all(TEST_DIR).unwrap();
        fs::remove_dir_all(TEST_DIR_OUT).unwrap();
    }

    #[test]
    #[cfg(target_family = "windows")]
    fn copy_symlink() {
        use std::os::windows::fs as wfs;
        use std::env;
        const TEST_DIR: &str = "test_copy_files_copy_symlink";
        const TEST_DIR_OUT: &str = "test_copy_files_copy_symlink_out_seq";
        let CURRENT_PATH: PathBuf = env::current_dir().unwrap();

        fs::create_dir_all(TEST_DIR).unwrap();
        fs::create_dir_all(TEST_DIR_OUT).unwrap();
        wfs::symlink_file("src/main.rs", [TEST_DIR, "file"].join("/")).unwrap();
        wfs::symlink_dir("src", [TEST_DIR, "dir"].join("/")).unwrap();

        copy_files(
            get_all_files(TEST_DIR).unwrap().symlinks().par_iter(),
            TEST_DIR,
            TEST_DIR_OUT,
        );

        let mut links_set = HashSet::new();
        links_set.insert(Symlink {
            path: PathBuf::from("file"),
            target: PathBuf::from("src/main.rs"),
        });

        links_set.insert(Symlink {
            path: PathBuf::from("dir"),
            target: PathBuf::from("src/"),
        });

        assert_eq!(
            get_all_files(TEST_DIR_OUT).unwrap(),
            FileSets {
                files: HashSet::new(),
                dirs: HashSet::new(),
                symlinks: links_set.clone(),
            }
        );

       fs::remove_dir_all(TEST_DIR).unwrap();
       fs::remove_dir_all(TEST_DIR_OUT).unwrap();
    }
}

#[cfg(test)]
mod test_compare_and_copy_files {
    use super::*;

    #[test]
    fn single_same() {
        const TEST_DIR: &str = "src";
        const TEST_DIR_OUT: &str = "test_compare_and_copy_files_single_same_out";

        fs::create_dir_all(TEST_DIR_OUT).unwrap();

        fs::copy(
            [TEST_DIR, "main.rs"].join("/"),
            [TEST_DIR_OUT, "main.rs"].join("/"),
        )
        .unwrap();

        let file_to_compare = File {
            path: PathBuf::from("main.rs"),
            size: fs::metadata([TEST_DIR, "main.rs"].join("/")).unwrap().len(),
        };

        let mut files_to_compare = HashSet::new();
        files_to_compare.insert(file_to_compare.clone());

        let mut flags = Flag::empty();
        flags |= Flag::SECURE;

        compare_and_copy_files(
            files_to_compare.clone().par_iter(),
            TEST_DIR,
            TEST_DIR_OUT,
            Flag::empty(),
        );

        compare_and_copy_files(files_to_compare.par_iter(), TEST_DIR, TEST_DIR_OUT, flags);

        let actual = fs::read([TEST_DIR_OUT, "main.rs"].join("/")).unwrap();
        let expected = fs::read([TEST_DIR, "main.rs"].join("/")).unwrap();
        assert_eq!(actual, expected);

        fs::remove_dir_all(TEST_DIR_OUT).unwrap();
    }

    #[test]
    fn single_different() {
        const TEST_DIR: &str = "src";
        const TEST_DIR_OUT: &str = "test_compare_and_copy_files_single_different_out";

        fs::create_dir_all(TEST_DIR_OUT).unwrap();
        fs::File::create([TEST_DIR_OUT, "main.rs"].join("/")).unwrap();

        let file_to_compare = File {
            path: PathBuf::from("main.rs"),
            size: fs::metadata([TEST_DIR, "main.rs"].join("/")).unwrap().len(),
        };
        let mut files_to_compare = HashSet::new();
        files_to_compare.insert(file_to_compare.clone());

        compare_and_copy_files(
            files_to_compare.par_iter(),
            TEST_DIR,
            TEST_DIR_OUT,
            Flag::empty(),
        );

        let actual = fs::read([TEST_DIR_OUT, "main.rs"].join("/")).unwrap();
        let expected = fs::read([TEST_DIR, "main.rs"].join("/")).unwrap();

        assert_eq!(actual, expected);

        fs::remove_dir_all(TEST_DIR_OUT).unwrap();
    }
}
