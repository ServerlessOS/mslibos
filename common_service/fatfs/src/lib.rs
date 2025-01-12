#![allow(clippy::result_unit_err)]

use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
    sync::Mutex,
};

use fscommon::BufStream;
use ms_hostcall::types::{Fd, OpenFlags, Size};
pub use ms_std;
use ms_std::libos::libos;

type FileSystem = fatfs::FileSystem<fscommon::BufStream<std::fs::File>>;
type File<'a> = fatfs::File<'a, fscommon::BufStream<std::fs::File>>;

fn get_fs_image_path() -> PathBuf {
    let image_path = match libos!(fs_image(ms_std::init_context::isolation_ctx().isol_id)) {
        Some(s) => s,
        None => "fs_images/fatfs.img".to_owned(),
    };

    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
        .join(image_path)
}

thread_local! {
    static FS_RAW: FileSystem = {
        let image = {
            let mut config = fs::File::options();
            let image_path = get_fs_image_path();
            BufStream::new(config
                .read(true)
                .write(true)
                .open(image_path.clone())
                .unwrap_or_else(|e| panic!("open img {:?} failed, err: {}", image_path, e)))
        };
        FileSystem::new(image, fatfs::FsOptions::new()).expect("fatfs::new() failed.")
    };

    static FTABLE: Mutex<Vec<Option<File<'static>>>> = Mutex::new(Vec::default());
}

fn get_fs_ref() -> &'static FileSystem {
    // I think this hack for getting reference to file system instance is
    // valid because thread local store can guarantee 'static lifetime.
    let fs_addr = FS_RAW.with(|fs| fs as *const _ as usize);
    unsafe { &*(fs_addr as *const FileSystem) }
}

fn get_file_mut(fd: Fd) -> &'static mut File<'static> {
    FTABLE.with(|ft| {
        let mut ft = ft.lock().expect("require lock failed.");
        if let Some(Some(file)) = ft.get_mut(fd as usize) {
            let file_addr = file as *const _ as usize;
            // println!("get_file_mut: file addr=0x{:x}", file_addr);
            unsafe { &mut *(file_addr as *mut File) }
        } else {
            panic!("fd don't exist");
        }
    })
}

#[no_mangle]
pub fn fatfs_read(fd: Fd, buf: &mut [u8]) -> Result<Size, ()> {
    let file = get_file_mut(fd);

    Ok(file.read(buf).expect("fatfs_read failed."))
}

#[no_mangle]
pub fn fatfs_open(p: &str, flags: OpenFlags) -> Result<Fd, ()> {
    let root_dir = get_fs_ref().root_dir();

    let file = if flags.contains(OpenFlags::O_CREAT) {
        root_dir.create_file(p).expect("create file failed.")
    } else {
        root_dir.open_file(p).expect("open file failed.")
    };

    let fd = FTABLE.with(|table| {
        let mut table = table.lock().expect("require lock failed.");
        table.push(Some(file));
        table.len() - 1
    });

    Ok(fd as u32)
}

#[test]
fn fatfs_open_test() {
    let fd = fatfs_open("new_file.txt", OpenFlags::O_CREAT).expect("open file failed") as usize;
    FTABLE.with(|t| {
        let mut t = t.lock().expect("require lock failed");
        assert!(t.len() == fd + 1);
        if let Some(Some(ref mut f)) = t.get_mut(fd) {
            let mut buf = String::new();
            f.read_to_string(&mut buf).expect("read failed");
            // assert!(!buf.is_empty());
        };
    })
}

#[no_mangle]
pub fn fatfs_write(fd: Fd, buf: &[u8]) -> Result<Size, ()> {
    let file = get_file_mut(fd);
    file.write_all(buf).expect("write file failed");
    file.flush().expect("flush failed");

    Ok(buf.len())
}

#[test]
fn fatfs_write_test() {
    let root_dir = get_fs_ref().root_dir();
    let mut file = root_dir
        .create_file("hello.txt")
        .expect("create file failed");

    assert!(file.write(b"Hello World!").expect("write failed") > 0);
}

#[no_mangle]
pub fn fatfs_close(fd: Fd) -> Result<(), ()> {
    let mut old_file = None;

    FTABLE.with(|ftable| {
        let mut ftable = ftable.lock().expect("require lock failed.");
        if (fd as usize) < ftable.len() {
            std::mem::swap(&mut ftable[fd as usize], &mut old_file)
        };
    });

    if let Some(file) = old_file {
        drop(file);
        Ok(())
    } else {
        Err(())
    }
}
