//! Shared folders: local folders the PC sees as drives under `\\tsclient`
//! (MS-RDPEFS), for the PCs the user allows them.
//!
//! The PC is a remote peer, so every path it sends is untrusted:
//! - it resolves inside the folder's root or is refused: no `..`, no root or
//!   drive prefix, and no symbolic link anywhere on the way, so nothing can
//!   point outside;
//! - a read-only folder refuses everything that would change it (writing,
//!   creating, renaming, deleting, changing times) as write-protected media;
//! - reads, open files and refused attempts reported are capped, so a PC
//!   cannot exhaust this computer's memory, files or log.
//!
//! The file operations are ported from RustConn's drive backend
//! (`rustconn-core`, GPL-3.0-or-later); the read-only mode and the limits are
//! Remoport's. Change notices follow RustConn's watcher, and never follow a
//! symbolic link, so nothing outside a folder is described.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use ironrdp_core::impl_as_any;
use ironrdp_pdu::PduResult;
use ironrdp_rdpdr::RdpdrBackend;
use ironrdp_rdpdr::pdu::RdpdrPdu;
use ironrdp_rdpdr::pdu::efs::{
    Boolean, ClientDriveNotifyChangeDirectoryResponse, ClientDriveQueryDirectoryResponse,
    ClientDriveQueryInformationResponse, ClientDriveQuerySecurityResponse,
    ClientDriveQueryVolumeInformationResponse, ClientDriveSetInformationResponse,
    ClientDriveSetSecurityResponse, CreateDisposition, CreateOptions, DesiredAccess,
    DeviceCloseRequest, DeviceCloseResponse, DeviceControlRequest, DeviceControlResponse,
    DeviceCreateRequest, DeviceCreateResponse, DeviceFlushBuffersResponse, DeviceIoRequest,
    DeviceIoResponse, DeviceReadRequest, DeviceReadResponse, DeviceWriteRequest,
    DeviceWriteResponse, FileAttributeTagInformation, FileAttributes, FileBasicInformation,
    FileBothDirectoryInformation, FileDirectoryInformation, FileFsAttributeInformation,
    FileFsFullSizeInformation, FileFsSizeInformation, FileFsVolumeInformation,
    FileFullDirectoryInformation, FileInformationClass, FileInformationClassLevel,
    FileNamesInformation, FileRenameInformation, FileStandardInformation, FileSystemAttributes,
    FileSystemInformationClass, FileSystemInformationClassLevel, Information, NtStatus,
    ServerDeviceAnnounceResponse, ServerDriveIoRequest, ServerDriveNotifyChangeDirectoryRequest,
    ServerDriveQueryDirectoryRequest, ServerDriveQueryInformationRequest,
    ServerDriveQueryVolumeInformationRequest, ServerDriveSetInformationRequest,
};
use ironrdp_rdpdr::pdu::esc::{ScardCall, ScardIoCtlCode};
use ironrdp_svc::SvcMessage;

use crate::{Event, SharedFolder};

/// Files and folders the PC may hold open at once.
const MAX_OPEN: usize = 1024;

/// The most bytes one read hands over; a shorter read is legal, and the PC
/// asks again for the rest.
const MAX_READ: u32 = 1024 * 1024;

/// Refused attempts written to the activity log in one session.
const MAX_REPORTED: u32 = 20;

/// Folders the PC may be waiting on for changes at once.
const MAX_WATCHES: usize = 64;

/// Access rights that change a file: the specific ones, and GENERIC_ALL and
/// GENERIC_WRITE, which the bit set does not name.
const MODIFYING_ACCESS: u32 = DesiredAccess::FILE_WRITE_DATA_OR_FILE_ADD_FILE.bits()
    | DesiredAccess::FILE_APPEND_DATA_OR_FILE_ADD_SUBDIRECTORY.bits()
    | DesiredAccess::FILE_WRITE_EA.bits()
    | DesiredAccess::FILE_WRITE_ATTRIBUTES.bits()
    | DesiredAccess::FILE_DELETE_CHILD.bits()
    | DesiredAccess::DELETE.bits()
    | DesiredAccess::WRITE_DAC.bits()
    | DesiredAccess::WRITE_OWNER.bits()
    | 0x1000_0000
    | 0x4000_0000;

/// Access rights that need the file opened for writing.
const WRITING_ACCESS: u32 = DesiredAccess::FILE_WRITE_DATA_OR_FILE_ADD_FILE.bits()
    | DesiredAccess::FILE_APPEND_DATA_OR_FILE_ADD_SUBDIRECTORY.bits()
    | DesiredAccess::FILE_WRITE_EA.bits()
    | DesiredAccess::FILE_WRITE_ATTRIBUTES.bits()
    | 0x1000_0000
    | 0x4000_0000;

#[derive(Debug)]
struct Share {
    /// The folder, resolved once at the start of the session.
    root: PathBuf,
    read_only: bool,
}

#[derive(Debug)]
struct OpenFile {
    file: File,
    path: PathBuf,
    device_id: u32,
    /// What is left of a directory listing, handed out one entry at a time.
    listing: Vec<(PathBuf, String)>,
    delete_on_close: bool,
}

/// A folder the PC waits on for its next change (MS-RDPEFS 2.2.3.3.11):
/// answered once, then the PC asks again.
#[derive(Debug)]
struct Watch {
    request: DeviceIoRequest,
    path: PathBuf,
    recursive: bool,
    filter: u32,
}

#[derive(Debug)]
pub(crate) struct DriveBackend {
    shares: HashMap<u32, Share>,
    open: HashMap<u32, OpenFile>,
    next_file_id: u32,
    events: mpsc::Sender<Event>,
    reported: u32,
    /// The PC has opened something in a shared folder.
    used: bool,
    /// Started at the first change the PC waits for.
    watcher: Option<(notify::RecommendedWatcher, mpsc::Receiver<notify::Event>)>,
    watches: HashMap<u32, Watch>,
}

impl_as_any!(DriveBackend);

impl DriveBackend {
    /// The backend for `folders`, and the drives to announce, by device ID.
    /// A folder that cannot be found is left out and reported.
    pub(crate) fn new(
        folders: &[SharedFolder],
        events: mpsc::Sender<Event>,
    ) -> (Self, Vec<(u32, String)>) {
        let mut shares = HashMap::new();
        let mut drives = Vec::new();
        for (device_id, folder) in (1..).zip(folders) {
            match std::fs::canonicalize(&folder.path) {
                Ok(root) if root.is_dir() => {
                    let name = root.file_name().map_or_else(
                        || "Folder".to_string(),
                        |n| n.to_string_lossy().into_owned(),
                    );
                    let _ = events.send(Event::Progress(format!(
                        "Sharing {} as \\\\tsclient\\{name}{}",
                        root.display(),
                        if folder.read_only {
                            " (read-only)"
                        } else {
                            " (read and write)"
                        }
                    )));
                    drives.push((device_id, name));
                    shares.insert(
                        device_id,
                        Share {
                            root,
                            read_only: folder.read_only,
                        },
                    );
                }
                _ => {
                    let _ = events.send(Event::Progress(format!(
                        "The shared folder {} was not found; it is not shared",
                        folder.path.display()
                    )));
                }
            }
        }
        let backend = Self {
            shares,
            open: HashMap::new(),
            next_file_id: 1,
            events,
            reported: 0,
            used: false,
            watcher: None,
            watches: HashMap::new(),
        };
        (backend, drives)
    }

    fn report(&mut self, message: String) {
        self.reported += 1;
        if self.reported <= MAX_REPORTED {
            let _ = self.events.send(Event::Progress(message));
        } else if self.reported == MAX_REPORTED + 1 {
            let _ = self.events.send(Event::Progress(
                "Further refused attempts on the shared folders are not logged".to_string(),
            ));
        }
    }

    fn alloc_file_id(&mut self) -> u32 {
        loop {
            let id = self.next_file_id;
            self.next_file_id = self.next_file_id.wrapping_add(1).max(1);
            if !self.open.contains_key(&id) {
                return id;
            }
        }
    }

    fn read_only(&self, device_id: u32) -> bool {
        self.shares
            .get(&device_id)
            .is_none_or(|share| share.read_only)
    }

    /// Resolves a path the PC sent beneath its shared folder, or refuses it.
    fn resolve(&mut self, device_id: u32, windows_path: &str) -> Result<PathBuf, NtStatus> {
        let Some(root) = self.shares.get(&device_id).map(|share| share.root.clone()) else {
            return Err(NtStatus::NO_SUCH_FILE);
        };
        // A leading backslash is the shared folder's root; everything after
        // it has to stay relative.
        let unix_path = windows_path.replace('\\', "/");
        let mut relative = PathBuf::new();
        for component in Path::new(unix_path.trim_start_matches('/')).components() {
            match component {
                Component::Normal(name) => relative.push(name),
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    self.report(format!(
                        "Refused: the PC asked for {windows_path:?}, outside the shared folder"
                    ));
                    return Err(NtStatus::ACCESS_DENIED);
                }
            }
        }
        // No symbolic link on the way: one could point anywhere, and a linked
        // parent would carry a create, rename or delete outside the folder.
        let mut current = root.clone();
        for component in relative.components() {
            current.push(component);
            match std::fs::symlink_metadata(&current) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    self.report(format!(
                        "Refused: {windows_path:?} goes through a symbolic link in the shared folder"
                    ));
                    return Err(NtStatus::ACCESS_DENIED);
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                Err(e) => return Err(io_error_to_status(&e)),
            }
        }
        Ok(root.join(relative))
    }

    fn create(&mut self, req: DeviceCreateRequest) -> PduResult<Vec<SvcMessage>> {
        let header = req.device_io_request.clone();
        let device_id = header.device_id;
        if self.open.len() >= MAX_OPEN {
            self.report(format!(
                "Refused: the PC tried to hold more than {MAX_OPEN} files open"
            ));
            return create_reply(
                header,
                NtStatus::INSUFFICIENT_RESOURCES,
                0,
                Information::empty(),
            );
        }
        let path = match self.resolve(device_id, &req.path) {
            Ok(path) => path,
            Err(status) => return create_reply(header, status, 0, Information::empty()),
        };
        let metadata = std::fs::metadata(&path);
        let exists = metadata.is_ok();
        let disposition = req.create_disposition;
        let delete_on_close = req
            .create_options
            .contains(CreateOptions::FILE_DELETE_ON_CLOSE);
        let creates = disposition == CreateDisposition::FILE_CREATE
            || disposition == CreateDisposition::FILE_OVERWRITE
            || disposition == CreateDisposition::FILE_OVERWRITE_IF
            || disposition == CreateDisposition::FILE_SUPERSEDE
            || (disposition == CreateDisposition::FILE_OPEN_IF && !exists);
        let read_only = self.read_only(device_id);
        if read_only
            && (creates || delete_on_close || req.desired_access.bits() & MODIFYING_ACCESS != 0)
        {
            self.report(format!(
                "Refused: the shared folder is read-only, and the PC tried to change {:?}",
                req.path
            ));
            return create_reply(
                header,
                NtStatus::MEDIA_WRITE_PROTECTED,
                0,
                Information::empty(),
            );
        }

        let directory = req
            .create_options
            .contains(CreateOptions::FILE_DIRECTORY_FILE);
        let (opened, information) = if directory {
            match &metadata {
                Ok(meta) if meta.is_dir() => (File::open(&path), Information::FILE_OPENED),
                Ok(_) => {
                    return create_reply(
                        header,
                        NtStatus::NOT_A_DIRECTORY,
                        0,
                        Information::empty(),
                    );
                }
                Err(_)
                    if disposition == CreateDisposition::FILE_CREATE
                        || disposition == CreateDisposition::FILE_OPEN_IF =>
                {
                    (
                        std::fs::create_dir(&path).and_then(|()| File::open(&path)),
                        Information::FILE_SUPERSEDED,
                    )
                }
                Err(_) => {
                    return create_reply(header, NtStatus::NO_SUCH_FILE, 0, Information::empty());
                }
            }
        } else {
            // Windows opens an existing file with FILE_OPEN even when it means
            // to write to it: the access asked for is the only signal.
            let writes = !read_only && req.desired_access.bits() & WRITING_ACCESS != 0;
            let mut options = OpenOptions::new();
            options.read(true);
            if !read_only {
                match disposition {
                    CreateDisposition::FILE_CREATE => {
                        options.write(true).create_new(true);
                    }
                    CreateDisposition::FILE_OPEN_IF => {
                        options.write(true).create(true);
                    }
                    CreateDisposition::FILE_OVERWRITE => {
                        options.write(true).truncate(true);
                    }
                    CreateDisposition::FILE_OVERWRITE_IF | CreateDisposition::FILE_SUPERSEDE => {
                        options.write(true).truncate(true).create(true);
                    }
                    _ => {
                        options.write(writes);
                    }
                }
            }
            // A file read-only on disk, or a directory, must still open for
            // the metadata queries Explorer runs before anything else.
            let opened = options
                .open(&path)
                .or_else(|e| if writes { File::open(&path) } else { Err(e) });
            let information = if disposition == CreateDisposition::FILE_CREATE {
                Information::FILE_SUPERSEDED
            } else if disposition == CreateDisposition::FILE_OVERWRITE
                || disposition == CreateDisposition::FILE_OVERWRITE_IF
            {
                Information::FILE_OVERWRITTEN
            } else {
                Information::FILE_OPENED
            };
            (opened, information)
        };

        match opened {
            Ok(file) => {
                if !self.used {
                    self.used = true;
                    let _ = self
                        .events
                        .send(Event::Progress("The PC opened a shared folder".to_string()));
                }
                let file_id = self.alloc_file_id();
                self.open.insert(
                    file_id,
                    OpenFile {
                        file,
                        path,
                        device_id,
                        listing: Vec::new(),
                        delete_on_close,
                    },
                );
                create_reply(header, NtStatus::SUCCESS, file_id, information)
            }
            Err(e) => create_reply(header, io_error_to_status(&e), 0, Information::empty()),
        }
    }

    fn close(&mut self, req: DeviceCloseRequest) -> PduResult<Vec<SvcMessage>> {
        self.stop_watching(req.device_io_request.file_id);
        if let Some(open) = self.open.remove(&req.device_io_request.file_id) {
            let OpenFile {
                file,
                path,
                delete_on_close,
                ..
            } = open;
            drop(file);
            if delete_on_close {
                let removed = if path.is_dir() {
                    std::fs::remove_dir(&path)
                } else {
                    std::fs::remove_file(&path)
                };
                if let Err(e) = removed {
                    self.report(format!("Could not delete {}: {e}", path.display()));
                }
            }
        }
        reply(RdpdrPdu::DeviceCloseResponse(DeviceCloseResponse {
            device_io_response: DeviceIoResponse::new(req.device_io_request, NtStatus::SUCCESS),
        }))
    }

    fn read(&mut self, req: DeviceReadRequest) -> PduResult<Vec<SvcMessage>> {
        let length = req.length.min(MAX_READ) as usize;
        let read = self
            .open
            .get_mut(&req.device_io_request.file_id)
            .ok_or(NtStatus::INVALID_HANDLE)
            .and_then(|open| {
                let mut buf = vec![0u8; length];
                open.file
                    .seek(SeekFrom::Start(req.offset))
                    .and_then(|_| open.file.read(&mut buf))
                    .map(|n| {
                        buf.truncate(n);
                        buf
                    })
                    .map_err(|e| io_error_to_status(&e))
            });
        let (status, read_data) = match read {
            Ok(data) => (NtStatus::SUCCESS, data),
            Err(status) => (status, Vec::new()),
        };
        reply(RdpdrPdu::DeviceReadResponse(DeviceReadResponse {
            device_io_reply: DeviceIoResponse::new(req.device_io_request, status),
            read_data,
        }))
    }

    fn write(&mut self, req: DeviceWriteRequest) -> PduResult<Vec<SvcMessage>> {
        let device_id = req.device_io_request.device_id;
        let written = if self.read_only(device_id) {
            Err(NtStatus::MEDIA_WRITE_PROTECTED)
        } else {
            self.open
                .get_mut(&req.device_io_request.file_id)
                .ok_or(NtStatus::INVALID_HANDLE)
                .and_then(|open| {
                    open.file
                        .seek(SeekFrom::Start(req.offset))
                        .and_then(|_| open.file.write(&req.write_data))
                        .map_err(|e| io_error_to_status(&e))
                })
        };
        let (status, length) = match written {
            Ok(n) => (NtStatus::SUCCESS, u32::try_from(n).unwrap_or(u32::MAX)),
            Err(status) => (status, 0),
        };
        reply(RdpdrPdu::DeviceWriteResponse(DeviceWriteResponse {
            device_io_reply: DeviceIoResponse::new(req.device_io_request, status),
            length,
        }))
    }

    fn query_information(
        &self,
        req: ServerDriveQueryInformationRequest,
    ) -> PduResult<Vec<SvcMessage>> {
        let answer = |status, buffer| {
            reply(RdpdrPdu::ClientDriveQueryInformationResponse(
                ClientDriveQueryInformationResponse {
                    device_io_response: DeviceIoResponse::new(
                        req.device_io_request.clone(),
                        status,
                    ),
                    buffer,
                },
            ))
        };
        let Some(open) = self.open.get(&req.device_io_request.file_id) else {
            return answer(NtStatus::INVALID_HANDLE, None);
        };
        let Ok(meta) = open.file.metadata() else {
            return answer(NtStatus::UNSUCCESSFUL, None);
        };
        let name = open
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let attributes = file_attributes(&meta, &name, self.read_only(open.device_id));
        let size = i64::try_from(meta.size()).unwrap_or(i64::MAX);
        let buffer = match req.file_info_class_lvl {
            FileInformationClassLevel::FILE_BASIC_INFORMATION => {
                FileInformationClass::Basic(FileBasicInformation {
                    creation_time: unix_to_filetime(meta.ctime()),
                    last_access_time: unix_to_filetime(meta.atime()),
                    last_write_time: unix_to_filetime(meta.mtime()),
                    change_time: unix_to_filetime(meta.ctime()),
                    file_attributes: attributes,
                })
            }
            FileInformationClassLevel::FILE_STANDARD_INFORMATION => {
                FileInformationClass::Standard(FileStandardInformation {
                    allocation_size: size,
                    end_of_file: size,
                    number_of_links: u32::try_from(meta.nlink()).unwrap_or(u32::MAX),
                    delete_pending: if open.delete_on_close {
                        Boolean::True
                    } else {
                        Boolean::False
                    },
                    directory: if meta.is_dir() {
                        Boolean::True
                    } else {
                        Boolean::False
                    },
                })
            }
            // Explorer asks this before every rename, delete or copy; an
            // empty success there aborts the operation. No reparse points are
            // ever shown, so the tag is 0.
            FileInformationClassLevel::FILE_ATTRIBUTE_TAG_INFORMATION => {
                FileInformationClass::AttributeTag(FileAttributeTagInformation {
                    file_attributes: attributes,
                    reparse_tag: 0,
                })
            }
            // An empty buffer with success is malformed: say it is not
            // supported, and Windows falls back.
            _ => return answer(NtStatus::NOT_SUPPORTED, None),
        };
        answer(NtStatus::SUCCESS, Some(buffer))
    }

    fn query_volume(
        &self,
        req: ServerDriveQueryVolumeInformationRequest,
    ) -> PduResult<Vec<SvcMessage>> {
        let device_id = req.device_io_request.device_id;
        let read_only = self.read_only(device_id);
        let root = self.shares.get(&device_id).map(|share| share.root.clone());
        let buffer = match req.fs_info_class_lvl {
            FileSystemInformationClassLevel::FILE_FS_ATTRIBUTE_INFORMATION => {
                let mut attributes = FileSystemAttributes::FILE_CASE_SENSITIVE_SEARCH
                    | FileSystemAttributes::FILE_CASE_PRESERVED_NAMES
                    | FileSystemAttributes::FILE_UNICODE_ON_DISK;
                if read_only {
                    attributes |= FileSystemAttributes::FILE_READ_ONLY_VOLUME;
                }
                Some(FileSystemInformationClass::FileFsAttributeInformation(
                    FileFsAttributeInformation {
                        file_system_attributes: attributes,
                        max_component_name_len: 255,
                        file_system_name: "Remoport".to_owned(),
                    },
                ))
            }
            FileSystemInformationClassLevel::FILE_FS_VOLUME_INFORMATION => Some(
                FileSystemInformationClass::FileFsVolumeInformation(FileFsVolumeInformation {
                    volume_creation_time: unix_to_filetime(0),
                    volume_serial_number: 0x5250_0001 + device_id,
                    supports_objects: Boolean::False,
                    volume_label: "Remoport".to_owned(),
                }),
            ),
            FileSystemInformationClassLevel::FILE_FS_SIZE_INFORMATION => {
                let (total, available) = disk_units(root.as_deref());
                Some(FileSystemInformationClass::FileFsSizeInformation(
                    FileFsSizeInformation {
                        total_alloc_units: total,
                        available_alloc_units: available,
                        sectors_per_alloc_unit: 8,
                        bytes_per_sector: 512,
                    },
                ))
            }
            FileSystemInformationClassLevel::FILE_FS_FULL_SIZE_INFORMATION => {
                let (total, available) = disk_units(root.as_deref());
                Some(FileSystemInformationClass::FileFsFullSizeInformation(
                    FileFsFullSizeInformation {
                        total_alloc_units: total,
                        caller_available_alloc_units: available,
                        actual_available_alloc_units: available,
                        sectors_per_alloc_unit: 8,
                        bytes_per_sector: 512,
                    },
                ))
            }
            _ => None,
        };
        let status = if buffer.is_some() {
            NtStatus::SUCCESS
        } else {
            NtStatus::NOT_SUPPORTED
        };
        reply(RdpdrPdu::ClientDriveQueryVolumeInformationResponse(
            ClientDriveQueryVolumeInformationResponse {
                device_io_reply: DeviceIoResponse::new(req.device_io_request, status),
                buffer,
            },
        ))
    }

    fn query_directory(
        &mut self,
        req: ServerDriveQueryDirectoryRequest,
    ) -> PduResult<Vec<SvcMessage>> {
        let file_id = req.device_io_request.file_id;
        let root = self
            .shares
            .get(&req.device_io_request.device_id)
            .map(|share| share.root.clone());
        let Some(open) = self.open.get_mut(&file_id) else {
            return directory_reply(req.device_io_request, NtStatus::INVALID_HANDLE, None);
        };
        if req.initial_query > 0 {
            let pattern = search_pattern(&req.path);
            let path = open.path.clone();
            let mut listing = Vec::new();
            // Windows listings include these two; at the folder's root `..`
            // points back at the root, so nothing outside it is described.
            for pseudo in [".", ".."] {
                if wildcard_match(pattern, pseudo) {
                    let target = if pseudo == "." {
                        path.clone()
                    } else {
                        path.parent()
                            .filter(|parent| root.as_ref().is_some_and(|r| parent.starts_with(r)))
                            .map_or_else(|| path.clone(), Path::to_path_buf)
                    };
                    listing.push((target, pseudo.to_owned()));
                }
            }
            if let Ok(directory) = std::fs::read_dir(&path) {
                let mut entries: Vec<(PathBuf, String)> = directory
                    .filter_map(Result::ok)
                    // Links cannot be opened, so they are not listed either.
                    .filter(|entry| entry.file_type().is_ok_and(|kind| !kind.is_symlink()))
                    .filter_map(|entry| {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        wildcard_match(pattern, &name).then(|| (entry.path(), name))
                    })
                    .collect();
                entries.sort_by_key(|(_, name)| name.to_lowercase());
                listing.extend(entries);
            }
            // Handed out from the end: reverse once so the order holds.
            listing.reverse();
            open.listing = listing;
        }
        let read_only = self.read_only(req.device_io_request.device_id);
        let Some(open) = self.open.get_mut(&file_id) else {
            return directory_reply(req.device_io_request, NtStatus::INVALID_HANDLE, None);
        };
        while let Some((path, name)) = open.listing.pop() {
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if let Some(buffer) =
                directory_information(&req.file_info_class_lvl, &meta, name, read_only)
            {
                return directory_reply(req.device_io_request, NtStatus::SUCCESS, Some(buffer));
            }
        }
        let status = if req.initial_query > 0 {
            NtStatus::NO_SUCH_FILE
        } else {
            NtStatus::NO_MORE_FILES
        };
        directory_reply(req.device_io_request, status, None)
    }

    fn set_information(
        &mut self,
        req: &ServerDriveSetInformationRequest,
    ) -> PduResult<Vec<SvcMessage>> {
        let file_id = req.device_io_request.file_id;
        let device_id = req.device_io_request.device_id;
        let status = if self.read_only(device_id) {
            self.report(
                "Refused: the shared folder is read-only, and the PC tried to change a file"
                    .to_string(),
            );
            NtStatus::MEDIA_WRITE_PROTECTED
        } else if !self.open.contains_key(&file_id) {
            NtStatus::INVALID_HANDLE
        } else {
            match &req.set_buffer {
                FileInformationClass::Rename(rename) => self.rename(file_id, device_id, rename),
                FileInformationClass::EndOfFile(eof) => self.truncate(file_id, eof.end_of_file),
                FileInformationClass::Allocation(alloc) => {
                    self.truncate(file_id, alloc.allocation_size)
                }
                FileInformationClass::Disposition(disposition) => {
                    // Removed on close, when the PC lets go of the file.
                    if let Some(open) = self.open.get_mut(&file_id) {
                        open.delete_on_close = disposition.delete_pending != 0;
                    }
                    NtStatus::SUCCESS
                }
                FileInformationClass::Basic(basic) => self.set_times(file_id, basic),
                _ => NtStatus::NOT_SUPPORTED,
            }
        };
        let response = match ClientDriveSetInformationResponse::new(req, status) {
            Ok(response) => response,
            Err(_) => ClientDriveSetInformationResponse::new(req, NtStatus::UNSUCCESSFUL)
                .map_err(|e| ironrdp_pdu::encode_err!(e))?,
        };
        reply(RdpdrPdu::ClientDriveSetInformationResponse(response))
    }

    /// Renames to another path inside the same folder. `std::fs::rename`
    /// replaces an existing target, so that is refused unless the PC asked.
    fn rename(&mut self, file_id: u32, device_id: u32, rename: &FileRenameInformation) -> NtStatus {
        let new_path = match self.resolve(device_id, &rename.file_name) {
            Ok(path) => path,
            Err(status) => return status,
        };
        let Some(open) = self.open.get_mut(&file_id) else {
            return NtStatus::INVALID_HANDLE;
        };
        if rename.replace_if_exists == Boolean::False
            && new_path != open.path
            && std::fs::exists(&new_path).unwrap_or(false)
        {
            return NtStatus::OBJECT_NAME_COLLISION;
        }
        match std::fs::rename(&open.path, &new_path) {
            Ok(()) => {
                open.path = new_path;
                NtStatus::SUCCESS
            }
            Err(e) => io_error_to_status(&e),
        }
    }

    fn truncate(&mut self, file_id: u32, size: i64) -> NtStatus {
        let Ok(size) = u64::try_from(size) else {
            return NtStatus::INVALID_PARAMETER;
        };
        match self
            .open
            .get_mut(&file_id)
            .map(|open| open.file.set_len(size))
        {
            Some(Ok(())) => NtStatus::SUCCESS,
            Some(Err(e)) => io_error_to_status(&e),
            None => NtStatus::INVALID_HANDLE,
        }
    }

    /// Sets the access and write times; 0 and -1 leave one as it is
    /// (MS-FSCC 2.4.7). Attributes are ignored: mapping them onto Unix
    /// permissions would change the user's files unexpectedly. A failure is
    /// not reported, since Windows stamps every copied file and failing that
    /// would abort the copy.
    fn set_times(&self, file_id: u32, basic: &FileBasicInformation) -> NtStatus {
        let Some(open) = self.open.get(&file_id) else {
            return NtStatus::INVALID_HANDLE;
        };
        let mut times = std::fs::FileTimes::new();
        if let Some(accessed) = filetime_to_system(basic.last_access_time) {
            times = times.set_accessed(accessed);
        }
        if let Some(modified) = filetime_to_system(basic.last_write_time) {
            times = times.set_modified(modified);
        }
        let _ = open.file.set_times(times);
        NtStatus::SUCCESS
    }
}

impl DriveBackend {
    /// Starts waiting on the folder `req` names for its next change.
    fn watch(&mut self, req: &ServerDriveNotifyChangeDirectoryRequest) -> Result<(), NtStatus> {
        use notify::Watcher as _;
        let file_id = req.device_io_request.file_id;
        let path = match self.open.get(&file_id) {
            Some(open) if open.path.is_dir() => open.path.clone(),
            Some(_) => return Err(NtStatus::NOT_A_DIRECTORY),
            None => return Err(NtStatus::INVALID_HANDLE),
        };
        if self.watches.len() >= MAX_WATCHES && !self.watches.contains_key(&file_id) {
            return Err(NtStatus::INSUFFICIENT_RESOURCES);
        }
        if self.watcher.is_none() {
            let (changes, received) = mpsc::channel();
            let watcher = notify::RecommendedWatcher::new(
                move |event: notify::Result<notify::Event>| {
                    if let Ok(event) = event {
                        let _ = changes.send(event);
                    }
                },
                notify::Config::default().with_follow_symlinks(false),
            )
            .map_err(|_| NtStatus::NOT_SUPPORTED)?;
            self.watcher = Some((watcher, received));
        }
        let recursive = req.watch_tree != 0;
        let mode = if recursive {
            notify::RecursiveMode::Recursive
        } else {
            notify::RecursiveMode::NonRecursive
        };
        if let Some((watcher, _)) = &mut self.watcher {
            watcher
                .watch(&path, mode)
                .map_err(|_| NtStatus::NOT_SUPPORTED)?;
        }
        self.watches.insert(
            file_id,
            Watch {
                request: req.device_io_request.clone(),
                path,
                recursive,
                filter: req.completion_filter,
            },
        );
        Ok(())
    }

    fn stop_watching(&mut self, file_id: u32) {
        use notify::Watcher as _;
        let Some(watch) = self.watches.remove(&file_id) else {
            return;
        };
        let still_used = self.watches.values().any(|w| w.path == watch.path);
        if let (false, Some((watcher, _))) = (still_used, &mut self.watcher) {
            let _ = watcher.unwatch(&watch.path);
        }
    }

    /// Answers each waiting folder that changed since the last call.
    fn changes(&mut self) -> Vec<SvcMessage> {
        let Some((_, received)) = &self.watcher else {
            return Vec::new();
        };
        let events: Vec<notify::Event> = received.try_iter().take(1024).collect();
        let mut answers = Vec::new();
        for event in events {
            let Some(action) = file_action(&event.kind) else {
                continue;
            };
            for path in &event.paths {
                let answered: Vec<u32> = self
                    .watches
                    .iter()
                    .filter(|(_, w)| covers(w, path) && filter_matches(w.filter, &event.kind))
                    .map(|(&file_id, _)| file_id)
                    .collect();
                for file_id in answered {
                    let Some(watch) = self.watches.get(&file_id) else {
                        continue;
                    };
                    let name = relative_name(&watch.path, path);
                    answers.push(SvcMessage::from(
                        RdpdrPdu::ClientDriveNotifyChangeDirectoryResponse(
                            ClientDriveNotifyChangeDirectoryResponse::new(
                                watch.request.clone(),
                                NtStatus::SUCCESS,
                                notify_information(action, &name),
                            ),
                        ),
                    ));
                    self.stop_watching(file_id);
                }
            }
        }
        answers
    }
}

/// Whether a change at `path` is one `watch` waits for.
fn covers(watch: &Watch, path: &Path) -> bool {
    if watch.recursive {
        path != watch.path && path.starts_with(&watch.path)
    } else {
        path.parent() == Some(watch.path.as_path())
    }
}

/// The FILE_ACTION_* code for a change (MS-FSCC 2.4.42).
fn file_action(kind: &notify::EventKind) -> Option<u32> {
    use notify::EventKind;
    use notify::event::{ModifyKind, RenameMode};
    match kind {
        EventKind::Create(_) => Some(1),
        EventKind::Remove(_) => Some(2),
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => Some(4),
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => Some(5),
        EventKind::Modify(_) => Some(3),
        _ => None,
    }
}

/// Whether the PC's completion filter asks for this kind of change
/// (MS-SMB2 2.2.35), as in RustConn.
fn filter_matches(filter: u32, kind: &notify::EventKind) -> bool {
    use notify::EventKind;
    use notify::event::ModifyKind;
    const FILE_NAME: u32 = 0x01;
    const DIR_NAME: u32 = 0x02;
    const ATTRIBUTES: u32 = 0x04;
    const SIZE: u32 = 0x08;
    const LAST_WRITE: u32 = 0x10;
    const LAST_ACCESS: u32 = 0x20;
    const CREATION: u32 = 0x40;
    const SECURITY: u32 = 0x100;
    let wanted = match kind {
        EventKind::Create(_) => FILE_NAME | DIR_NAME | CREATION,
        EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_)) => FILE_NAME | DIR_NAME,
        EventKind::Modify(ModifyKind::Data(_)) => SIZE | LAST_WRITE,
        EventKind::Modify(ModifyKind::Metadata(_)) => ATTRIBUTES | LAST_ACCESS | SECURITY,
        EventKind::Modify(_) => LAST_WRITE,
        _ => 0,
    };
    filter & wanted != 0
}

/// `path` relative to the watched folder, with Windows separators.
fn relative_name(folder: &Path, path: &Path) -> String {
    path.strip_prefix(folder)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('/', "\\")
}

/// One FILE_NOTIFY_INFORMATION entry (MS-FSCC 2.4.42), the last one.
fn notify_information(action: u32, name: &str) -> Vec<u8> {
    let name: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut entry = Vec::with_capacity(12 + name.len());
    entry.extend_from_slice(&0u32.to_le_bytes());
    entry.extend_from_slice(&action.to_le_bytes());
    entry.extend_from_slice(&u32::try_from(name.len()).unwrap_or(u32::MAX).to_le_bytes());
    entry.extend_from_slice(&name);
    entry
}

impl RdpdrBackend for DriveBackend {
    fn reset(&mut self) -> PduResult<()> {
        // A new initialization sequence: the PC's earlier handles are gone.
        self.open.clear();
        self.watches.clear();
        self.watcher = None;
        Ok(())
    }

    fn poll_deferred_messages(&mut self) -> PduResult<Vec<SvcMessage>> {
        Ok(self.changes())
    }

    fn handle_server_device_announce_response(
        &mut self,
        _pdu: ServerDeviceAnnounceResponse,
    ) -> PduResult<()> {
        Ok(())
    }

    fn handle_scard_call(
        &mut self,
        _req: DeviceControlRequest<ScardIoCtlCode>,
        _call: ScardCall,
    ) -> PduResult<Vec<SvcMessage>> {
        // No smart card is announced, so none is answered.
        Ok(Vec::new())
    }

    fn handle_drive_io_request(&mut self, req: ServerDriveIoRequest) -> PduResult<Vec<SvcMessage>> {
        match req {
            ServerDriveIoRequest::ServerCreateDriveRequest(req) => self.create(req),
            ServerDriveIoRequest::DeviceCloseRequest(req) => self.close(req),
            ServerDriveIoRequest::DeviceReadRequest(req) => self.read(req),
            ServerDriveIoRequest::DeviceWriteRequest(req) => self.write(req),
            ServerDriveIoRequest::ServerDriveQueryInformationRequest(req) => {
                self.query_information(req)
            }
            ServerDriveIoRequest::ServerDriveQueryVolumeInformationRequest(req) => {
                self.query_volume(req)
            }
            ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(req) => {
                self.query_directory(req)
            }
            ServerDriveIoRequest::ServerDriveSetInformationRequest(req) => {
                self.set_information(&req)
            }
            ServerDriveIoRequest::DeviceControlRequest(req) => {
                reply(RdpdrPdu::DeviceControlResponse(DeviceControlResponse {
                    device_io_reply: DeviceIoResponse::new(req.header, NtStatus::SUCCESS),
                    output_buffer: None,
                }))
            }
            ServerDriveIoRequest::ServerDriveNotifyChangeDirectoryRequest(req) => {
                match self.watch(&req) {
                    // Answered when the folder changes.
                    Ok(()) => Ok(Vec::new()),
                    Err(status) => reply(RdpdrPdu::ClientDriveNotifyChangeDirectoryResponse(
                        ClientDriveNotifyChangeDirectoryResponse::new(
                            req.device_io_request,
                            status,
                            Vec::new(),
                        ),
                    )),
                }
            }
            // Byte-range locks are acknowledged, not enforced, as in RustConn.
            ServerDriveIoRequest::ServerDriveLockControlRequest(req) => {
                let status = if self.open.contains_key(&req.device_io_request.file_id) {
                    NtStatus::SUCCESS
                } else {
                    NtStatus::INVALID_HANDLE
                };
                reply(RdpdrPdu::DeviceCloseResponse(DeviceCloseResponse {
                    device_io_response: DeviceIoResponse::new(req.device_io_request, status),
                }))
            }
            // Writes go straight to the file: nothing to flush.
            ServerDriveIoRequest::DeviceFlushBuffersRequest(req) => reply(
                RdpdrPdu::DeviceFlushBuffersResponse(DeviceFlushBuffersResponse {
                    device_io_response: DeviceIoResponse::new(
                        req.device_io_request,
                        NtStatus::SUCCESS,
                    ),
                }),
            ),
            // The folder carries no Windows security descriptors.
            ServerDriveIoRequest::ServerDriveQuerySecurityRequest(req) => reply(
                RdpdrPdu::ClientDriveQuerySecurityResponse(ClientDriveQuerySecurityResponse {
                    device_io_response: DeviceIoResponse::new(
                        req.device_io_request,
                        NtStatus::NOT_SUPPORTED,
                    ),
                    security_descriptor: None,
                }),
            ),
            ServerDriveIoRequest::ServerDriveSetSecurityRequest(req) => reply(
                RdpdrPdu::ClientDriveSetSecurityResponse(ClientDriveSetSecurityResponse {
                    device_io_response: DeviceIoResponse::new(
                        req.device_io_request,
                        NtStatus::NOT_SUPPORTED,
                    ),
                    length: 0,
                }),
            ),
        }
    }
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "every handler answers through the trait's PduResult"
)]
fn reply(pdu: RdpdrPdu) -> PduResult<Vec<SvcMessage>> {
    Ok(vec![SvcMessage::from(pdu)])
}

fn create_reply(
    header: DeviceIoRequest,
    status: NtStatus,
    file_id: u32,
    information: Information,
) -> PduResult<Vec<SvcMessage>> {
    reply(RdpdrPdu::DeviceCreateResponse(DeviceCreateResponse {
        device_io_reply: DeviceIoResponse::new(header, status),
        file_id,
        information,
    }))
}

fn directory_reply(
    header: DeviceIoRequest,
    status: NtStatus,
    buffer: Option<FileInformationClass>,
) -> PduResult<Vec<SvcMessage>> {
    reply(RdpdrPdu::ClientDriveQueryDirectoryResponse(
        ClientDriveQueryDirectoryResponse {
            device_io_reply: DeviceIoResponse::new(header, status),
            buffer,
        },
    ))
}

/// The file name pattern at the end of a directory query.
fn search_pattern(request_path: &str) -> &str {
    let pattern = request_path
        .trim_end_matches(['\\', '/'])
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or("*");
    if pattern.is_empty() || pattern == "*.*" {
        "*"
    } else {
        pattern
    }
}

/// Windows' `*` and `?` wildcards, without regard to case.
fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let value: Vec<char> = value.to_lowercase().chars().collect();
    let mut previous = vec![false; value.len() + 1];
    previous[0] = true;
    for token in pattern {
        let mut current = vec![false; value.len() + 1];
        if token == '*' {
            current[0] = previous[0];
            for index in 0..value.len() {
                current[index + 1] = previous[index + 1] || current[index];
            }
        } else {
            for (index, character) in value.iter().enumerate() {
                current[index + 1] = previous[index] && (token == '?' || token == *character);
            }
        }
        previous = current;
    }
    previous[value.len()]
}

/// One directory entry in the information class the PC asked for.
fn directory_information(
    level: &FileInformationClassLevel,
    meta: &std::fs::Metadata,
    name: String,
    read_only: bool,
) -> Option<FileInformationClass> {
    let created = unix_to_filetime(meta.ctime());
    let accessed = unix_to_filetime(meta.atime());
    let written = unix_to_filetime(meta.mtime());
    let size = i64::try_from(meta.size()).unwrap_or(i64::MAX);
    let attributes = file_attributes(meta, &name, read_only);
    Some(match *level {
        FileInformationClassLevel::FILE_DIRECTORY_INFORMATION => {
            FileInformationClass::Directory(FileDirectoryInformation::new(
                created, accessed, written, created, size, attributes, name,
            ))
        }
        FileInformationClassLevel::FILE_FULL_DIRECTORY_INFORMATION => {
            FileInformationClass::FullDirectory(FileFullDirectoryInformation::new(
                created, accessed, written, created, size, attributes, name,
            ))
        }
        FileInformationClassLevel::FILE_BOTH_DIRECTORY_INFORMATION => {
            FileInformationClass::BothDirectory(FileBothDirectoryInformation::new(
                created, accessed, written, created, size, attributes, name,
            ))
        }
        FileInformationClassLevel::FILE_NAMES_INFORMATION => {
            FileInformationClass::Names(FileNamesInformation::new(name))
        }
        _ => return None,
    })
}

/// The folder's disk as (total, available) units of 4096 bytes, matching
/// the 8 × 512-byte sectors reported; nominal sizes when it cannot be read.
fn disk_units(root: Option<&Path>) -> (i64, i64) {
    const UNIT: u64 = 4096;
    let Some(stat) = root.and_then(|root| nix::sys::statvfs::statvfs(root).ok()) else {
        return (1_000_000, 500_000);
    };
    #[allow(clippy::useless_conversion, reason = "u32 on 32-bit targets")]
    let fragment = u64::from(stat.fragment_size());
    #[allow(clippy::useless_conversion, reason = "u32 on 32-bit targets")]
    let (blocks, available) = (u64::from(stat.blocks()), u64::from(stat.blocks_available()));
    let units =
        |count: u64| i64::try_from(count.saturating_mul(fragment) / UNIT).unwrap_or(i64::MAX);
    (units(blocks), units(available))
}

/// The closest NTSTATUS for a file system error: Explorer shows it.
fn io_error_to_status(e: &std::io::Error) -> NtStatus {
    match e.kind() {
        std::io::ErrorKind::NotFound => NtStatus::NO_SUCH_FILE,
        std::io::ErrorKind::PermissionDenied => NtStatus::ACCESS_DENIED,
        std::io::ErrorKind::AlreadyExists => NtStatus::OBJECT_NAME_COLLISION,
        std::io::ErrorKind::DirectoryNotEmpty => NtStatus::DIRECTORY_NOT_EMPTY,
        std::io::ErrorKind::NotADirectory => NtStatus::NOT_A_DIRECTORY,
        _ => NtStatus::UNSUCCESSFUL,
    }
}

/// 100-nanosecond intervals from 1601 to 1970.
const FILETIME_UNIX_EPOCH: i64 = 116_444_736_000_000_000;

const fn unix_to_filetime(seconds: i64) -> i64 {
    seconds
        .saturating_mul(10_000_000)
        .saturating_add(FILETIME_UNIX_EPOCH)
}

/// A FILETIME as a time this computer can set; `None` for "leave as it is"
/// (0 or -1) and for anything before 1970.
fn filetime_to_system(filetime: i64) -> Option<SystemTime> {
    let since_unix = u64::try_from(filetime.checked_sub(FILETIME_UNIX_EPOCH)?).ok()?;
    if filetime <= 0 {
        return None;
    }
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_nanos(since_unix.checked_mul(100)?))
}

fn file_attributes(meta: &std::fs::Metadata, name: &str, read_only: bool) -> FileAttributes {
    let mut attributes = if meta.is_dir() {
        FileAttributes::FILE_ATTRIBUTE_DIRECTORY
    } else {
        FileAttributes::FILE_ATTRIBUTE_ARCHIVE
    };
    if name.starts_with('.') && name != "." && name != ".." {
        attributes |= FileAttributes::FILE_ATTRIBUTE_HIDDEN;
    }
    if read_only || meta.permissions().readonly() {
        attributes |= FileAttributes::FILE_ATTRIBUTE_READONLY;
    }
    attributes
}

#[cfg(test)]
mod tests {
    use ironrdp_rdpdr::pdu::efs::{
        FileDispositionInformation, FileEndOfFileInformation, MajorFunction, MinorFunction,
        SharedAccess,
    };

    use super::*;

    /// `IoStatus` inside the encoded `DR_DEVICE_IOCOMPLETION`.
    const STATUS_OFFSET: usize = 12;
    /// The file ID of a Create response, after the completion header.
    const FILE_ID_OFFSET: usize = 16;

    fn header(file_id: u32, major_function: MajorFunction) -> DeviceIoRequest {
        DeviceIoRequest {
            device_id: 1,
            file_id,
            completion_id: 0,
            major_function,
            minor_function: MinorFunction::from(0),
        }
    }

    fn u32_at(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("4 bytes"))
    }

    fn backend(dir: &Path, read_only: bool) -> DriveBackend {
        let folders = [SharedFolder {
            path: dir.to_path_buf(),
            read_only,
        }];
        let (backend, drives) = DriveBackend::new(&folders, mpsc::channel().0);
        assert_eq!(drives.len(), 1);
        backend
    }

    fn send(backend: &mut DriveBackend, request: ServerDriveIoRequest) -> Vec<u8> {
        let responses = backend.handle_drive_io_request(request).expect("answered");
        assert_eq!(responses.len(), 1);
        responses[0].encode_unframed_pdu().expect("encodes")
    }

    /// Opens `path` and returns the status and the file ID.
    fn open(
        backend: &mut DriveBackend,
        path: &str,
        access: DesiredAccess,
        disposition: CreateDisposition,
        options: CreateOptions,
    ) -> (u32, u32) {
        let bytes = send(
            backend,
            ServerDriveIoRequest::ServerCreateDriveRequest(DeviceCreateRequest {
                device_io_request: header(0, MajorFunction::Create),
                desired_access: access,
                allocation_size: 0,
                file_attributes: FileAttributes::empty(),
                shared_access: SharedAccess::empty(),
                create_disposition: disposition,
                create_options: options,
                path: path.to_owned(),
            }),
        );
        (
            u32_at(&bytes, STATUS_OFFSET),
            u32_at(&bytes, FILE_ID_OFFSET),
        )
    }

    fn open_to_write(backend: &mut DriveBackend, path: &str) -> (u32, u32) {
        open(
            backend,
            path,
            DesiredAccess::FILE_READ_DATA_OR_FILE_LIST_DIRECTORY
                | DesiredAccess::FILE_WRITE_DATA_OR_FILE_ADD_FILE,
            CreateDisposition::FILE_OPEN,
            CreateOptions::empty(),
        )
    }

    fn set(backend: &mut DriveBackend, file_id: u32, buffer: FileInformationClass) -> u32 {
        let bytes = send(
            backend,
            ServerDriveIoRequest::ServerDriveSetInformationRequest(
                ServerDriveSetInformationRequest {
                    device_io_request: header(file_id, MajorFunction::SetInformation),
                    set_buffer: buffer,
                },
            ),
        );
        u32_at(&bytes, STATUS_OFFSET)
    }

    fn close(backend: &mut DriveBackend, file_id: u32) {
        send(
            backend,
            ServerDriveIoRequest::DeviceCloseRequest(DeviceCloseRequest {
                device_io_request: header(file_id, MajorFunction::Close),
            }),
        );
    }

    #[test]
    fn paths_cannot_leave_the_folder() {
        let share = tempfile::tempdir().expect("share");
        let outside = tempfile::tempdir().expect("outside");
        std::os::unix::fs::symlink(outside.path(), share.path().join("escape")).expect("link");
        let mut backend = backend(share.path(), false);

        for path in [
            "\\..\\secret",
            "\\a\\..\\..\\secret",
            "\\escape\\secret",
            "\\escape",
        ] {
            assert!(
                matches!(backend.resolve(1, path), Err(NtStatus::ACCESS_DENIED)),
                "{path} was allowed"
            );
        }
        // A device the PC was never given is refused too.
        assert!(backend.resolve(2, "\\file").is_err());
        let root = std::fs::canonicalize(share.path()).expect("root");
        assert_eq!(
            backend.resolve(1, "\\safe\\file.txt").ok(),
            Some(root.join("safe/file.txt"))
        );
        assert_eq!(backend.resolve(1, "\\").ok(), Some(root));
    }

    #[test]
    fn links_inside_the_folder_are_not_listed() {
        let share = tempfile::tempdir().expect("share");
        std::fs::write(share.path().join("file.txt"), b"x").expect("file");
        std::os::unix::fs::symlink("/etc", share.path().join("etc")).expect("link");
        let mut backend = backend(share.path(), true);
        let (status, dir) = open(
            &mut backend,
            "\\",
            DesiredAccess::FILE_READ_DATA_OR_FILE_LIST_DIRECTORY,
            CreateDisposition::FILE_OPEN,
            CreateOptions::FILE_DIRECTORY_FILE,
        );
        assert_eq!(status, 0);
        let mut names = Vec::new();
        for initial in [1, 0, 0, 0, 0] {
            let bytes = send(
                &mut backend,
                ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(
                    ServerDriveQueryDirectoryRequest {
                        device_io_request: header(dir, MajorFunction::DirectoryControl),
                        file_info_class_lvl: FileInformationClassLevel::FILE_NAMES_INFORMATION,
                        initial_query: initial,
                        path: "\\*".to_owned(),
                    },
                ),
            );
            if u32_at(&bytes, STATUS_OFFSET) != 0 {
                break;
            }
            let name: Vec<u16> = bytes[FILE_ID_OFFSET + 4 + 12..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect();
            names.push(
                String::from_utf16_lossy(&name)
                    .trim_end_matches('\0')
                    .to_string(),
            );
        }
        assert_eq!(names, vec![".", "..", "file.txt"]);
    }

    #[test]
    fn the_log_says_when_a_folder_is_used_and_what_it_refused() {
        let share = tempfile::tempdir().expect("share");
        std::fs::write(share.path().join("doc.txt"), b"x").expect("file");
        let (events, received) = mpsc::channel();
        let folders = [SharedFolder {
            path: share.path().to_path_buf(),
            read_only: true,
        }];
        let (mut backend, _) = DriveBackend::new(&folders, events);
        assert_eq!(open_to_write(&mut backend, "\\doc.txt\n").0, 0xC000_00A2);
        for _ in 0..2 {
            let status = open(
                &mut backend,
                "\\doc.txt",
                DesiredAccess::FILE_READ_DATA_OR_FILE_LIST_DIRECTORY,
                CreateDisposition::FILE_OPEN,
                CreateOptions::empty(),
            );
            assert_eq!(status.0, 0);
        }
        let log: Vec<String> = received
            .try_iter()
            .filter_map(|e| match e {
                Event::Progress(line) => Some(line),
                _ => None,
            })
            .collect();
        // A path from the PC goes in escaped: it cannot forge a line of the log.
        assert!(
            log.iter()
                .any(|l| l.contains("read-only") && l.contains(r"doc.txt\n")),
            "{log:?}"
        );
        assert_eq!(
            log.iter()
                .filter(|l| l.contains("opened a shared folder"))
                .count(),
            1
        );
    }

    #[test]
    fn a_read_only_folder_cannot_be_changed() {
        let share = tempfile::tempdir().expect("share");
        std::fs::write(share.path().join("doc.txt"), b"original").expect("file");
        let mut backend = backend(share.path(), true);
        const WRITE_PROTECTED: u32 = 0xC000_00A2;

        assert_eq!(open_to_write(&mut backend, "\\doc.txt").0, WRITE_PROTECTED);
        for disposition in [
            CreateDisposition::FILE_CREATE,
            CreateDisposition::FILE_OVERWRITE_IF,
            CreateDisposition::FILE_SUPERSEDE,
            CreateDisposition::FILE_OPEN_IF,
        ] {
            let read = DesiredAccess::FILE_READ_DATA_OR_FILE_LIST_DIRECTORY;
            let status = open(
                &mut backend,
                "\\new.txt",
                read,
                disposition,
                CreateOptions::empty(),
            )
            .0;
            assert_eq!(status, WRITE_PROTECTED, "{disposition:?}");
        }
        let delete = open(
            &mut backend,
            "\\doc.txt",
            DesiredAccess::DELETE,
            CreateDisposition::FILE_OPEN,
            CreateOptions::empty(),
        );
        assert_eq!(delete.0, WRITE_PROTECTED);

        // Reading works, and nothing that changes the file gets through.
        let (status, file_id) = open(
            &mut backend,
            "\\doc.txt",
            DesiredAccess::FILE_READ_DATA_OR_FILE_LIST_DIRECTORY,
            CreateDisposition::FILE_OPEN,
            CreateOptions::empty(),
        );
        assert_eq!(status, 0);
        let rename = FileInformationClass::Rename(FileRenameInformation {
            replace_if_exists: Boolean::True,
            file_name: "\\moved.txt".to_owned(),
        });
        assert_eq!(set(&mut backend, file_id, rename), WRITE_PROTECTED);
        let truncate = FileInformationClass::EndOfFile(FileEndOfFileInformation { end_of_file: 0 });
        assert_eq!(set(&mut backend, file_id, truncate), WRITE_PROTECTED);
        let dispose =
            FileInformationClass::Disposition(FileDispositionInformation { delete_pending: 1 });
        assert_eq!(set(&mut backend, file_id, dispose), WRITE_PROTECTED);
        let bytes = send(
            &mut backend,
            ServerDriveIoRequest::DeviceWriteRequest(DeviceWriteRequest {
                device_io_request: header(file_id, MajorFunction::Write),
                offset: 0,
                write_data: b"changed".to_vec(),
            }),
        );
        assert_eq!(u32_at(&bytes, STATUS_OFFSET), WRITE_PROTECTED);
        close(&mut backend, file_id);
        assert_eq!(
            std::fs::read(share.path().join("doc.txt")).expect("read"),
            b"original"
        );
        assert!(!share.path().join("new.txt").exists());
    }

    #[test]
    fn a_writable_folder_renames_truncates_and_deletes_inside_itself() {
        let share = tempfile::tempdir().expect("share");
        std::fs::write(share.path().join("probe.txt"), b"payload").expect("file");
        std::fs::write(share.path().join("taken.txt"), b"other").expect("file");
        let mut backend = backend(share.path(), false);
        let (status, file_id) = open_to_write(&mut backend, "\\probe.txt");
        assert_eq!(status, 0);

        let escape = FileInformationClass::Rename(FileRenameInformation {
            replace_if_exists: Boolean::True,
            file_name: "\\..\\escaped.txt".to_owned(),
        });
        assert_eq!(set(&mut backend, file_id, escape), 0xC000_0022);
        let clobber = FileInformationClass::Rename(FileRenameInformation {
            replace_if_exists: Boolean::False,
            file_name: "\\taken.txt".to_owned(),
        });
        assert_eq!(set(&mut backend, file_id, clobber), 0xC000_0035);
        let rename = FileInformationClass::Rename(FileRenameInformation {
            replace_if_exists: Boolean::False,
            file_name: "\\renamed.txt".to_owned(),
        });
        assert_eq!(set(&mut backend, file_id, rename), 0);
        let truncate = FileInformationClass::EndOfFile(FileEndOfFileInformation { end_of_file: 3 });
        assert_eq!(set(&mut backend, file_id, truncate), 0);
        assert_eq!(
            std::fs::read(share.path().join("renamed.txt")).expect("read"),
            b"pay"
        );

        let dispose =
            FileInformationClass::Disposition(FileDispositionInformation { delete_pending: 1 });
        assert_eq!(set(&mut backend, file_id, dispose), 0);
        assert!(
            share.path().join("renamed.txt").exists(),
            "deleted before close"
        );
        close(&mut backend, file_id);
        assert!(!share.path().join("renamed.txt").exists());
        assert_eq!(
            std::fs::read(share.path().join("taken.txt")).expect("read"),
            b"other"
        );
    }

    #[test]
    fn reads_and_open_files_are_capped() {
        let share = tempfile::tempdir().expect("share");
        std::fs::write(
            share.path().join("big.bin"),
            vec![7u8; 2 * MAX_READ as usize],
        )
        .expect("file");
        let mut backend = backend(share.path(), true);
        let read_access = || DesiredAccess::FILE_READ_DATA_OR_FILE_LIST_DIRECTORY;
        let (_, file_id) = open(
            &mut backend,
            "\\big.bin",
            read_access(),
            CreateDisposition::FILE_OPEN,
            CreateOptions::empty(),
        );
        let bytes = send(
            &mut backend,
            ServerDriveIoRequest::DeviceReadRequest(DeviceReadRequest {
                device_io_request: header(file_id, MajorFunction::Read),
                length: u32::MAX,
                offset: 0,
            }),
        );
        assert_eq!(u32_at(&bytes, STATUS_OFFSET), 0);
        assert_eq!(u32_at(&bytes, 16), MAX_READ, "read length");

        for _ in 1..MAX_OPEN {
            let status = open(
                &mut backend,
                "\\big.bin",
                read_access(),
                CreateDisposition::FILE_OPEN,
                CreateOptions::empty(),
            )
            .0;
            assert_eq!(status, 0);
        }
        let refused = open(
            &mut backend,
            "\\big.bin",
            read_access(),
            CreateDisposition::FILE_OPEN,
            CreateOptions::empty(),
        );
        assert_eq!(refused.0, 0xC000_009A, "INSUFFICIENT_RESOURCES");
    }

    #[test]
    fn a_change_in_a_watched_folder_is_reported_once() {
        let share = tempfile::tempdir().expect("share");
        let mut backend = backend(share.path(), true);
        let (status, dir) = open(
            &mut backend,
            "\\",
            DesiredAccess::FILE_READ_DATA_OR_FILE_LIST_DIRECTORY,
            CreateDisposition::FILE_OPEN,
            CreateOptions::FILE_DIRECTORY_FILE,
        );
        assert_eq!(status, 0);
        let waiting = backend
            .handle_drive_io_request(
                ServerDriveIoRequest::ServerDriveNotifyChangeDirectoryRequest(
                    ServerDriveNotifyChangeDirectoryRequest {
                        device_io_request: header(dir, MajorFunction::DirectoryControl),
                        watch_tree: 0,
                        completion_filter: 0x01,
                    },
                ),
            )
            .expect("answered");
        assert!(waiting.is_empty(), "answered before anything changed");

        std::fs::write(share.path().join("new.txt"), b"x").expect("file");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let answers = loop {
            let answers = backend.poll_deferred_messages().expect("polled");
            if !answers.is_empty() || std::time::Instant::now() > deadline {
                break answers;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(answers.len(), 1, "one answer for the one change");
        let bytes = answers[0].encode_unframed_pdu().expect("encodes");
        assert_eq!(u32_at(&bytes, STATUS_OFFSET), 0);
        let name: Vec<u8> = "new.txt"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert!(bytes.windows(name.len()).any(|w| w == name.as_slice()));
        // Answered once: a further change waits for the PC to ask again.
        std::fs::write(share.path().join("other.txt"), b"y").expect("file");
        std::thread::sleep(Duration::from_millis(200));
        assert!(backend.poll_deferred_messages().expect("polled").is_empty());
    }

    #[test]
    fn a_missing_folder_is_not_shared() {
        let (events, received) = mpsc::channel();
        let folders = [SharedFolder {
            path: PathBuf::from("/nonexistent/remoport-test"),
            read_only: true,
        }];
        let (_, drives) = DriveBackend::new(&folders, events);
        assert!(drives.is_empty());
        assert!(
            received
                .try_iter()
                .any(|e| matches!(e, Event::Progress(m) if m.contains("not found")))
        );
    }

    #[test]
    fn windows_wildcards_ignore_case() {
        assert!(wildcard_match("*.TXT", "report.txt"));
        assert!(wildcard_match("file?.log", "File1.log"));
        assert!(!wildcard_match("file?.log", "file10.log"));
        assert_eq!(search_pattern("\\folder\\*.*"), "*");
    }

    #[test]
    fn times_before_1970_or_unset_are_left_alone() {
        assert_eq!(filetime_to_system(0), None);
        assert_eq!(filetime_to_system(-1), None);
        assert_eq!(filetime_to_system(FILETIME_UNIX_EPOCH - 1), None);
        assert_eq!(
            filetime_to_system(unix_to_filetime(86_400)),
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(86_400))
        );
    }
}
