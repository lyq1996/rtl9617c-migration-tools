// SPDX-License-Identifier: GPL-2.0-only

#[cfg(target_os = "linux")]
mod linux_app {
    use axum::body::Body;
    use axum::extract::{DefaultBodyLimit, Multipart, Path};
    use axum::http::{HeaderValue, StatusCode, header};
    use axum::response::{Html, IntoResponse, Response};
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use rtl9617c_migration_tools::{display, fip, install, mtd, unlock};
    use serde::Serialize;
    use std::io;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tokio::io::AsyncReadExt;
    use tokio_util::io::ReaderStream;

    const INDEX_HTML: &str = include_str!("index.html");
    const MAX_UPLOAD_SIZE: usize = fip::BOOT_SIZE + 1024 * 1024;
    static FLASH_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

    #[derive(Serialize)]
    struct ApiMessage {
        message: String,
    }

    fn html_escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    fn partition_table(partitions: &[mtd::Partition]) -> String {
        if partitions.is_empty() {
            return "<p class=\"hint\" data-t=\"noPartitions\">No MTD partitions found.</p>"
                .to_string();
        }

        let mut html = String::from(
            "<table><thead><tr><th data-t=\"device\">Device</th><th data-t=\"name\">Name</th><th data-t=\"size\">Size</th><th data-t=\"eraseBlock\">Erase block</th><th data-t=\"physicalOffset\">Physical offset</th><th data-t=\"writable\">Writable</th><th></th></tr></thead><tbody>",
        );
        for partition in partitions {
            let offset = partition
                .offset
                .map(|value| format!("0x{value:08x}"))
                .unwrap_or_else(|| "—".to_string());
            let writable = if partition.writeable() {
                "<span class=\"yes\" data-t=\"yes\">yes</span>"
            } else {
                "<span class=\"no\" data-t=\"no\">no</span>"
            };
            let download = if partition.is_ubi_volume() {
                "<span class=\"muted\" data-t=\"insideUbi\">in ubi_device</span>".to_string()
            } else {
                format!(
                    "<a data-t=\"download\" href=\"/backup/{}\">Download</a>",
                    partition.index
                )
            };
            html.push_str(&format!(
                "<tr><td>mtd{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                partition.index,
                html_escape(&partition.name),
                display::format_size(partition.size),
                display::format_size(partition.erase_size),
                offset,
                writable,
                download,
            ));
        }
        html.push_str("</tbody></table>");
        html
    }

    fn no_store(response: &mut Response) {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }

    fn text_response(status: StatusCode, message: impl Into<String>) -> Response {
        let mut response = (status, message.into()).into_response();
        no_store(&mut response);
        response
    }

    fn json_response(status: StatusCode, message: impl Into<String>) -> Response {
        let mut response = Json(ApiMessage {
            message: message.into(),
        })
        .into_response();
        *response.status_mut() = status;
        no_store(&mut response);
        response
    }

    async fn index() -> Response {
        let partitions = mtd::discover_partitions().unwrap_or_default();
        let board = install::resolve_board(&partitions)
            .map(|board| format!("{} ({})", board.name, board.compatible))
            .unwrap_or_else(|| "unidentified".to_string());
        let page = INDEX_HTML
            .replace("{{VERSION}}", env!("CARGO_PKG_VERSION"))
            .replace("{{BUILD}}", env!("BUILD_STAMP"))
            .replace("{{BOARD}}", &html_escape(&board))
            .replace("{{PARTITIONS}}", &partition_table(&partitions));
        let mut response = Html(page).into_response();
        no_store(&mut response);
        response
    }

    fn download_name(name: &str, prefix: &str) -> String {
        format!("{prefix}-{name}.bin")
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                    character
                } else {
                    '_'
                }
            })
            .collect()
    }

    fn file_response(file: tokio::fs::File, length: u64, disposition: String) -> Response {
        let stream = ReaderStream::with_capacity(file.take(length), 64 * 1024);
        let mut response = Response::new(Body::from_stream(stream));
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        response.headers_mut().insert(
            header::CONTENT_LENGTH,
            HeaderValue::from_str(&length.to_string()).unwrap(),
        );
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&disposition).unwrap(),
        );
        no_store(&mut response);
        response
    }

    async fn download_backup(Path(index): Path<u32>) -> Response {
        let partitions = match mtd::discover_partitions() {
            Ok(partitions) => partitions,
            Err(error) => {
                return text_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Reading the MTD partition table failed: {error}"),
                );
            }
        };
        let Some(partition) = partitions
            .into_iter()
            .find(|partition| partition.index == index)
        else {
            return text_response(StatusCode::NOT_FOUND, "MTD partition not found");
        };
        if partition.is_ubi_volume() {
            return text_response(
                StatusCode::CONFLICT,
                format!(
                    "{} is a UBI volume, not a partition. Download ubi_device instead.",
                    partition.name
                ),
            );
        }

        let file = match tokio::fs::File::open(&partition.path).await {
            Ok(file) => file,
            Err(error) => {
                return text_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Opening {} failed: {error}", partition.path.display()),
                );
            }
        };

        let disposition = format!(
            "attachment; filename=\"{}\"",
            download_name(&partition.name, &format!("mtd{}", partition.index))
        );
        file_response(file, partition.size, disposition)
    }

    async fn download_pre_flash() -> Response {
        let Ok(metadata) = tokio::fs::metadata(install::BACKUP_PATH).await else {
            return text_response(
                StatusCode::NOT_FOUND,
                "No pre-flash backup yet; it is created when an install starts",
            );
        };
        let file = match tokio::fs::File::open(install::BACKUP_PATH).await {
            Ok(file) => file,
            Err(error) => {
                return text_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Opening {} failed: {error}", install::BACKUP_PATH),
                );
            }
        };
        let disposition = format!("attachment; filename=\"{}\"", install::backup_filename());
        file_response(file, metadata.len(), disposition)
    }

    fn schedule_reboot() {
        tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(800)).await;
            unsafe {
                libc::sync();
                if libc::reboot(libc::RB_AUTOBOOT) != 0 {
                    eprintln!("Reboot failed: {}", io::Error::last_os_error());
                    FLASH_IN_PROGRESS.store(false, Ordering::Release);
                }
            }
        });
    }

    async fn flash(mut multipart: Multipart) -> Response {
        let mut upload = None;

        loop {
            let field = match multipart.next_field().await {
                Ok(Some(field)) => field,
                Ok(None) => break,
                Err(error) => {
                    return json_response(
                        StatusCode::BAD_REQUEST,
                        format!("Parsing the upload failed: {error}"),
                    );
                }
            };
            if field.name() != Some("image") {
                continue;
            }
            match field.bytes().await {
                Ok(data) => upload = Some(data.to_vec()),
                Err(error) => {
                    return json_response(
                        StatusCode::BAD_REQUEST,
                        format!("Reading the uploaded file failed: {error}"),
                    );
                }
            }
        }

        let Some(upload) = upload.filter(|data| !data.is_empty()) else {
            return json_response(StatusCode::BAD_REQUEST, "Select a U-Boot image");
        };
        if FLASH_IN_PROGRESS
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return json_response(StatusCode::CONFLICT, "A flash operation is already running");
        }

        let result = tokio::task::spawn_blocking(move || install::flash(&upload)).await;
        match result {
            Ok(Ok(message)) => {
                schedule_reboot();
                json_response(StatusCode::OK, message)
            }
            Ok(Err(error)) => {
                FLASH_IN_PROGRESS.store(false, Ordering::Release);
                json_response(StatusCode::BAD_REQUEST, error)
            }
            Err(error) => {
                FLASH_IN_PROGRESS.store(false, Ordering::Release);
                json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Flash task exited unexpectedly: {error}"),
                )
            }
        }
    }

    fn parse_arguments() -> Result<String, String> {
        let mut listen = "0.0.0.0:3333".to_string();
        let mut arguments = std::env::args().skip(1);
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--listen" => {
                    listen = arguments
                        .next()
                        .ok_or_else(|| "--listen requires an address".to_string())?;
                }
                "--version" | "-V" => {
                    println!("{}", build_identity());
                    std::process::exit(0);
                }
                "--help" | "-h" => {
                    println!(
                        "usage: rtl9617c-migration-tools [--listen 0.0.0.0:3333]\n\n  \
                         --version   print the version and build stamp"
                    );
                    std::process::exit(0);
                }
                _ => return Err(format!("Unknown argument: {argument}")),
            }
        }
        Ok(listen)
    }

    fn build_identity() -> String {
        format!(
            "rtl9617c-migration-tools {} (built {})",
            env!("CARGO_PKG_VERSION"),
            env!("BUILD_STAMP")
        )
    }

    fn prepare_mtd() {
        if unsafe { libc::geteuid() } != 0 {
            eprintln!("warning: not running as root; flashing will be refused");
            return;
        }
        let (writeable, total) = mtd::writeable_summary();
        if total > 0 && writeable == total {
            println!("MTD partitions are already writable; no module needed");
            return;
        }

        let release = match unlock::running_release() {
            Ok(release) => release,
            Err(error) => {
                eprintln!("{error}");
                return;
            }
        };
        match unlock::load(&release) {
            Ok(()) => {
                let (writeable, total) = mtd::writeable_summary();
                println!(
                    "MTD write protection cleared with the {release} module \
                     ({writeable}/{total} partitions writable)"
                );
                if writeable == 0 {
                    eprintln!("warning: no MTD partition is writable; flashing will fail");
                }
            }
            Err(error) => {
                eprintln!("MTD unlock failed: {error}");
                if writeable == 0 {
                    eprintln!("warning: no MTD partition is writable; flashing will fail");
                }
            }
        }
    }

    fn report() {
        let Ok(partitions) = mtd::discover_partitions() else {
            eprintln!("Reading /proc/mtd failed");
            return;
        };
        match install::resolve_board(&partitions) {
            Some(board) => println!("board: {} ({})", board.name, board.compatible),
            None => eprintln!(
                "warning: this unit's UBI volume table was not recognised; \
                 a bare u-boot.bin will be refused"
            ),
        }
        match mtd::find_boot_partition(&partitions) {
            Ok(partition) => println!(
                "boot partition: /dev/mtd{} ({}, eraseblock {}, {})",
                partition.index,
                display::format_size(partition.size),
                display::format_size(partition.erase_size),
                if partition.writeable() {
                    "writable"
                } else {
                    "read-only"
                }
            ),
            Err(error) => eprintln!("{error}"),
        }
        match unlock::running_release() {
            Ok(release) => println!(
                "kernel {release}; MTD modules bundled for {}",
                unlock::bundled().join(", ")
            ),
            Err(error) => eprintln!("{error}"),
        }
    }

    pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
        let listen = parse_arguments()?;
        println!("{}", build_identity());
        prepare_mtd();
        report();
        let app = Router::new()
            .route("/", get(index))
            .route("/backup/pre-flash", get(download_pre_flash))
            .route("/backup/{index}", get(download_backup))
            .route("/api/flash", post(flash))
            .layer(DefaultBodyLimit::max(MAX_UPLOAD_SIZE));
        let listener = tokio::net::TcpListener::bind(&listen).await?;
        println!("RTL9617C migration tools listening on http://{listen}");
        axum::serve(listener, app).await?;
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn partition(index: u32, name: &str) -> mtd::Partition {
            mtd::Partition {
                index,
                name: name.to_string(),
                size: 0x20_0000,
                erase_size: 0x2_0000,
                offset: Some(0),
                flags: None,
                path: std::path::PathBuf::from(format!("/dev/mtd{index}")),
            }
        }

        #[test]
        fn offers_downloads_only_for_physical_partitions() {
            let table = partition_table(&[
                partition(0, "boot"),
                partition(4, "ubi_device"),
                partition(5, "ubi_Config"),
                partition(9, "ubi_r0"),
            ]);
            assert!(table.contains("/backup/0"));
            assert!(table.contains("/backup/4"));
            assert!(!table.contains("/backup/5"));
            assert!(!table.contains("/backup/9"));
            assert!(table.contains("in ubi_device"));
        }
    }
}

#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    linux_app::run().await
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("rtl9617c-migration-tools requires Linux MTD character devices");
}
