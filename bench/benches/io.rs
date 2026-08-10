#![allow(missing_docs)]

extern crate alloc;

use alloc::sync::Arc;
use core::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use criterion::{
    Bencher, BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main,
};
use indoc::{formatdoc, indoc};
use stak::{
    device::VoidDevice,
    file::{FileSystem, OsFileSystem, VoidFileSystem},
    process_context::VoidProcessContext,
    r7rs::{SmallError, SmallPrimitiveSet},
    time::VoidClock,
    vm::{Heap, Memory, PrimitiveSet, Vm},
};
use stak_compiler::compile_r7rs;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{SyncSender, sync_channel},
    thread::{self, JoinHandle},
    time::Instant,
};
use tempfile::{TempDir, tempdir};

const HEAP_SIZE: usize = 1 << 22;
const SIZES: &[usize] = &[10_000, 100_000];

const BENCHMARK_START_PRIMITIVE: usize = 999;

struct BenchmarkPrimitiveSet<F: FileSystem> {
    inner: SmallPrimitiveSet<VoidDevice, F, VoidProcessContext, VoidClock>,
    ready: SyncSender<Result<(), String>>,
    resume: std::sync::mpsc::Receiver<()>,
    started: Arc<AtomicBool>,
    operation_start: Option<Instant>,
}

impl<F: FileSystem, H: Heap> PrimitiveSet<H> for BenchmarkPrimitiveSet<F> {
    type Error = SmallError;

    fn operate(&mut self, memory: &mut Memory<H>, primitive: usize) -> Result<(), Self::Error> {
        if primitive == BENCHMARK_START_PRIMITIVE {
            self.started.store(true, Ordering::Release);
            self.ready.send(Ok(())).unwrap();
            self.resume.recv().unwrap();
            self.operation_start = Some(Instant::now());
            memory.push(memory.boolean(false)?.into())?;
        } else {
            self.inner.operate(memory, primitive)?;
        }

        Ok(())
    }
}

struct PreparedRun {
    resume: SyncSender<()>,
    handle: JoinHandle<(Result<(), SmallError>, Option<Duration>)>,
}

impl PreparedRun {
    fn run(self) -> Result<Duration, SmallError> {
        self.resume.send(()).unwrap();
        let (result, elapsed) = self.handle.join().unwrap();

        result?;
        Ok(elapsed.expect("VM completed without passing the benchmark gate"))
    }
}

fn prepare_run<F: FileSystem + Send + 'static>(bytecode: Arc<[u8]>, file_system: F) -> PreparedRun {
    let (ready, wait_until_ready) = sync_channel(0);
    let (resume, wait_until_resumed) = sync_channel(0);
    let started = Arc::new(AtomicBool::new(false));
    let thread_started = started.clone();
    let failure = ready.clone();
    let handle = thread::spawn(move || {
        let mut elapsed = None;
        let result = (|| {
            let mut vm = Vm::new(
                vec![Default::default(); HEAP_SIZE],
                BenchmarkPrimitiveSet {
                    inner: SmallPrimitiveSet::new(
                        VoidDevice::new(),
                        file_system,
                        VoidProcessContext::new(),
                        VoidClock::new(),
                    ),
                    ready,
                    resume: wait_until_resumed,
                    started: thread_started.clone(),
                    operation_start: None,
                },
            )?;

            let result = vm.run(bytecode.iter().copied());
            elapsed = vm
                .primitive_set()
                .operation_start
                .map(|start| start.elapsed());
            result
        })();

        if !thread_started.load(Ordering::Acquire) {
            let message = match &result {
                Ok(()) => "VM completed before the benchmark gate".into(),
                Err(error) => format!("VM failed before the benchmark gate: {error:?}"),
            };
            let _ = failure.send(Err(message));
        }

        (result, elapsed)
    });
    wait_until_ready.recv().unwrap().unwrap();

    PreparedRun { resume, handle }
}

fn bench_prepared<F: FileSystem + Send + 'static>(
    bencher: &mut Bencher,
    bytecode: Arc<[u8]>,
    file_system: impl Fn() -> F,
) {
    bencher.iter_custom(|iterations| {
        (0..iterations).fold(Duration::ZERO, |elapsed, _| {
            elapsed + prepare_run(bytecode.clone(), file_system()).run().unwrap()
        })
    });
}

fn compile(source: &str) -> Vec<u8> {
    let mut bytecode = vec![];

    compile_r7rs(source.as_bytes(), &mut bytecode).unwrap();

    bytecode
}

fn run(bytecode: &[u8], file_system: impl FileSystem) -> Result<(), SmallError> {
    let mut vm = Vm::new(
        vec![Default::default(); HEAP_SIZE],
        SmallPrimitiveSet::new(
            VoidDevice::new(),
            file_system,
            VoidProcessContext::new(),
            VoidClock::new(),
        ),
    )?;

    vm.run(bytecode.iter().copied())
}

fn memory_source(operation: &str, size: usize) -> Vec<u8> {
    compile(&formatdoc!(
        "
        (import (scheme base))

        (define size {size})

        {operation}
        "
    ))
}

fn prepared_memory_source(preparation: &str, operation: &str, size: usize) -> Arc<[u8]> {
    compile(&formatdoc!(
        "
        (import (scheme base) (only (stak base) primitive))

        (define benchmark-start (primitive {BENCHMARK_START_PRIMITIVE}))
        (define size {size})

        {preparation}
        (benchmark-start)
        {operation}
        "
    ))
    .into()
}

fn file_source(operation: &str, size: usize, path: &Path) -> Vec<u8> {
    compile(&formatdoc!(
        r#"
        (import (scheme base) (scheme file))

        (define size {size})
        (define path "{}")

        {operation}
        "#,
        path.display()
    ))
}

fn prepared_file_source(preparation: &str, operation: &str, size: usize, path: &Path) -> Arc<[u8]> {
    compile(&formatdoc!(
        r#"
        (import
          (scheme base)
          (scheme file)
          (only (stak base) primitive))

        (define benchmark-start (primitive {BENCHMARK_START_PRIMITIVE}))
        (define size {size})
        (define path "{}")

        {preparation}
        (benchmark-start)
        {operation}
        "#,
        path.display(),
    ))
    .into()
}

fn create_input_file(directory: &TempDir, input: impl AsRef<[u8]>) -> PathBuf {
    let path = directory.path().join("input");
    fs::write(&path, input).unwrap();
    path
}

fn create_binary_input_file(directory: &TempDir, size: usize) -> PathBuf {
    create_input_file(directory, vec![65; size])
}

fn create_utf8_input_file(directory: &TempDir, size: usize) -> PathBuf {
    create_input_file(directory, "é".as_bytes().repeat(size))
}

fn bench_memory_ports(criterion: &mut Criterion) {
    const GROUP: &str = "io/in-memory-port";

    {
        let mut group = criterion.benchmark_group(format!("{GROUP}/prepare"));

        for &size in SIZES {
            for (name, operation) in [
                (
                    "read-bytevector",
                    indoc!(
                        "
                        (define source (make-bytevector size 65))
                        (define port (open-input-bytevector source))
                        "
                    ),
                ),
                (
                    "write-bytevector",
                    indoc!(
                        "
                        (define source (make-bytevector size 65))
                        (define port (open-output-bytevector))
                        "
                    ),
                ),
                (
                    "read-string",
                    indoc!(
                        r"
                        (define source (make-string size #\a))
                        (define port (open-input-string source))
                        "
                    ),
                ),
                (
                    "write-string",
                    indoc!(
                        r"
                        (define source (make-string size #\a))
                        (define port (open-output-string))
                        "
                    ),
                ),
            ] {
                let bytecode = memory_source(operation, size);

                group.bench_function(BenchmarkId::new(name, size), |bencher| {
                    bencher.iter(|| {
                        run(black_box(&bytecode), VoidFileSystem::new()).unwrap();
                    })
                });
            }
        }
    }

    let mut group = criterion.benchmark_group(GROUP);

    for &size in SIZES {
        group.throughput(Throughput::Bytes(size as _));

        for (name, preparation, operation) in [
            (
                "read-bytevector",
                indoc!(
                    "
                    (define source (make-bytevector size 65))
                    (define port (open-input-bytevector source))
                    "
                ),
                indoc!(
                    "
                    (read-bytevector size port)
                    "
                ),
            ),
            (
                "write-bytevector",
                indoc!(
                    "
                    (define source (make-bytevector size 65))
                    (define port (open-output-bytevector))
                    "
                ),
                indoc!(
                    "
                    (write-bytevector source port)
                    (get-output-bytevector port)
                    "
                ),
            ),
            (
                "read-string",
                indoc!(
                    r"
                    (define source (make-string size #\a))
                    (define port (open-input-string source))
                    "
                ),
                indoc!(
                    "
                    (read-string size port)
                    "
                ),
            ),
            (
                "write-string",
                indoc!(
                    r"
                    (define source (make-string size #\a))
                    (define port (open-output-string))
                    "
                ),
                indoc!(
                    "
                    (write-string source port)
                    (get-output-string port)
                    "
                ),
            ),
        ] {
            let bytecode = prepared_memory_source(preparation, operation, size);

            group.bench_function(BenchmarkId::new(name, size), |bencher| {
                bench_prepared(bencher, bytecode.clone(), VoidFileSystem::new)
            });
        }
    }
}

fn bench_memory_utf8_ports(criterion: &mut Criterion) {
    const GROUP: &str = "io/in-memory-port/utf8";

    {
        let mut group = criterion.benchmark_group(format!("{GROUP}/prepare"));

        for &size in SIZES {
            for (name, operation) in [
                (
                    "read-string",
                    indoc!(
                        r"
                        (define source (make-string size #\é))
                        (define port (open-input-string source))
                        "
                    ),
                ),
                (
                    "write-string",
                    indoc!(
                        r"
                        (define source (make-string size #\é))
                        (define port (open-output-string))
                        "
                    ),
                ),
            ] {
                let bytecode = memory_source(operation, size);

                group.bench_function(BenchmarkId::new(name, size), |bencher| {
                    bencher.iter(|| {
                        run(black_box(&bytecode), VoidFileSystem::new()).unwrap();
                    })
                });
            }
        }
    }

    let mut group = criterion.benchmark_group(GROUP);

    for &size in SIZES {
        group.throughput(Throughput::Bytes((size * 2) as _));

        for (name, preparation, operation) in [
            (
                "read-string",
                indoc!(
                    r"
                    (define source (make-string size #\é))
                    (define port (open-input-string source))
                    "
                ),
                indoc!(
                    "
                    (read-string size port)
                    "
                ),
            ),
            (
                "write-string",
                indoc!(
                    r"
                    (define source (make-string size #\é))
                    (define port (open-output-string))
                    "
                ),
                indoc!(
                    "
                    (write-string source port)
                    (get-output-string port)
                    "
                ),
            ),
        ] {
            let bytecode = prepared_memory_source(preparation, operation, size);

            group.bench_function(BenchmarkId::new(name, size), |bencher| {
                bench_prepared(bencher, bytecode.clone(), VoidFileSystem::new)
            });
        }
    }
}

fn bench_os_files(criterion: &mut Criterion) {
    const GROUP: &str = "io/os-file";

    let directory = tempdir().unwrap();

    {
        let mut group = criterion.benchmark_group(format!("{GROUP}/prepare"));

        for &size in SIZES {
            for (name, operation) in [
                (
                    "read-bytevector",
                    indoc!(
                        "
                        (define port (open-input-file path))
                        (close-input-port port)
                        "
                    ),
                ),
                (
                    "read-string",
                    indoc!(
                        "
                        (define port (open-input-file path))
                        (close-input-port port)
                        "
                    ),
                ),
            ] {
                let bytecode =
                    file_source(operation, size, &create_binary_input_file(&directory, size));

                group.bench_function(BenchmarkId::new(name, size), |bencher| {
                    bencher.iter(|| {
                        run(black_box(&bytecode), OsFileSystem::new()).unwrap();
                    })
                });
            }

            for (name, operation) in [
                (
                    "write-bytevector",
                    indoc!(
                        "
                        (define source (make-bytevector size 65))
                        (define port (open-output-file path))
                        (close-output-port port)
                        "
                    ),
                ),
                (
                    "write-string",
                    indoc!(
                        r"
                        (define source (make-string size #\a))
                        (define port (open-output-file path))
                        (close-output-port port)
                        "
                    ),
                ),
            ] {
                let bytecode = file_source(operation, size, &directory.path().join("output"));

                group.bench_function(BenchmarkId::new(name, size), |bencher| {
                    bencher.iter(|| {
                        run(black_box(&bytecode), OsFileSystem::new()).unwrap();
                    })
                });
            }
        }
    }

    let mut group = criterion.benchmark_group(GROUP);

    for &size in SIZES {
        group.throughput(Throughput::Bytes(size as _));

        let input = create_binary_input_file(&directory, size);
        for (name, operation) in [
            (
                "read-bytevector",
                indoc!(
                    "
                    (read-bytevector size port)
                    (close-input-port port)
                    "
                ),
            ),
            (
                "read-string",
                indoc!(
                    "
                    (read-string size port)
                    (close-input-port port)
                    "
                ),
            ),
        ] {
            let preparation = indoc!(
                "
                (define port (open-input-file path))
                "
            );
            let bytecode = prepared_file_source(preparation, operation, size, &input);

            group.bench_function(BenchmarkId::new(name, size), |bencher| {
                bench_prepared(bencher, bytecode.clone(), OsFileSystem::new)
            });
        }

        for (name, preparation, operation) in [
            (
                "write-bytevector",
                indoc!(
                    "
                    (define source (make-bytevector size 65))
                    (define port (open-output-file path))
                    "
                ),
                indoc!(
                    "
                    (write-bytevector source port)
                    (close-output-port port)
                    "
                ),
            ),
            (
                "write-string",
                indoc!(
                    r"
                    (define source (make-string size #\a))
                    (define port (open-output-file path))
                    "
                ),
                indoc!(
                    "
                    (write-string source port)
                    (close-output-port port)
                    "
                ),
            ),
        ] {
            let bytecode = prepared_file_source(
                preparation,
                operation,
                size,
                &directory.path().join("output"),
            );

            group.bench_function(BenchmarkId::new(name, size), |bencher| {
                bench_prepared(bencher, bytecode.clone(), OsFileSystem::new)
            });
        }
    }
}

fn bench_os_utf8_files(criterion: &mut Criterion) {
    const GROUP: &str = "io/os-file/utf8";

    let directory = tempdir().unwrap();

    {
        let mut group = criterion.benchmark_group(format!("{GROUP}/prepare"));

        for &size in SIZES {
            let bytecode = file_source(
                indoc!(
                    "
                    (define port (open-input-file path))
                    (close-input-port port)
                    "
                ),
                size,
                &create_utf8_input_file(&directory, size),
            );

            group.bench_function(BenchmarkId::new("read-string", size), |bencher| {
                bencher.iter(|| {
                    run(black_box(&bytecode), OsFileSystem::new()).unwrap();
                })
            });

            let bytecode = file_source(
                indoc!(
                    r"
                    (define source (make-string size #\é))
                    (define port (open-output-file path))
                    (close-output-port port)
                    "
                ),
                size,
                &directory.path().join("output"),
            );

            group.bench_function(BenchmarkId::new("write-string", size), |bencher| {
                bencher.iter(|| {
                    run(black_box(&bytecode), OsFileSystem::new()).unwrap();
                })
            });
        }
    }

    let mut group = criterion.benchmark_group(GROUP);

    for &size in SIZES {
        group.throughput(Throughput::Bytes((size * 2) as _));

        let bytecode = prepared_file_source(
            indoc!(
                "
                (define port (open-input-file path))
                "
            ),
            indoc!(
                "
                (read-string size port)
                (close-input-port port)
                "
            ),
            size,
            &create_utf8_input_file(&directory, size),
        );

        group.bench_function(BenchmarkId::new("read-string", size), |bencher| {
            bench_prepared(bencher, bytecode.clone(), OsFileSystem::new)
        });

        let bytecode = prepared_file_source(
            indoc!(
                r"
                (define source (make-string size #\é))
                (define port (open-output-file path))
                "
            ),
            indoc!(
                "
                (write-string source port)
                (close-output-port port)
                "
            ),
            size,
            &directory.path().join("output"),
        );

        group.bench_function(BenchmarkId::new("write-string", size), |bencher| {
            bench_prepared(bencher, bytecode.clone(), OsFileSystem::new)
        });
    }
}

criterion_group!(
    benches,
    bench_memory_ports,
    bench_memory_utf8_ports,
    bench_os_files,
    bench_os_utf8_files,
);
criterion_main!(benches);
