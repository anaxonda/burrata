//! Local temporary-storage facts, scoped to one reader and retained across cached seeks.
//! Neither error messages nor filesystem paths cross this boundary.
use std::{
    io::{self, Read, Seek, SeekFrom, Write},
    sync::{Arc, Mutex},
};
use stream_download::storage::StorageProvider;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Operation {
    Create,
    Read,
    Write,
    Flush,
    Seek,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    QuotaExceeded,
    NoSpace,
    PermissionDenied,
    Other,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fault {
    pub operation: Operation,
    pub kind: Kind,
}
#[derive(Clone, Default)]
pub struct Faults(Arc<Mutex<Option<Fault>>>);
impl Faults {
    pub fn get(&self) -> Option<Fault> {
        *self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn record(&self, operation: Operation, error: &io::Error) {
        // Interrupted is retryable and is not evidence of a failed storage operation.
        if error.kind() == io::ErrorKind::Interrupted {
            return;
        }
        let kind = match error.kind() {
            io::ErrorKind::QuotaExceeded => Kind::QuotaExceeded,
            io::ErrorKind::StorageFull => Kind::NoSpace,
            io::ErrorKind::PermissionDenied => Kind::PermissionDenied,
            _ => Kind::Other,
        };
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert(Fault { operation, kind });
    }
    fn result<T>(&self, operation: Operation, result: io::Result<T>) -> io::Result<T> {
        if let Err(error) = &result {
            self.record(operation, error);
        }
        result
    }
}
#[derive(Clone)]
pub struct Observed<P> {
    pub inner: P,
    pub faults: Faults,
}
pub struct Handle<T> {
    inner: T,
    faults: Faults,
}
impl<P: StorageProvider> StorageProvider for Observed<P> {
    type Reader = Handle<P::Reader>;
    type Writer = Handle<P::Writer>;
    fn max_capacity(&self) -> Option<usize> {
        self.inner.max_capacity()
    }
    fn into_reader_writer(self, length: Option<u64>) -> io::Result<(Self::Reader, Self::Writer)> {
        let (reader, writer) = self
            .faults
            .result(Operation::Create, self.inner.into_reader_writer(length))?;
        Ok((
            Handle {
                inner: reader,
                faults: self.faults.clone(),
            },
            Handle {
                inner: writer,
                faults: self.faults,
            },
        ))
    }
}
impl<T: Read> Read for Handle<T> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.faults.result(Operation::Read, self.inner.read(bytes))
    }
}
impl<T: Write> Write for Handle<T> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.faults
            .result(Operation::Write, self.inner.write(bytes))
    }
    fn flush(&mut self) -> io::Result<()> {
        self.faults.result(Operation::Flush, self.inner.flush())
    }
}
impl<T: Seek> Seek for Handle<T> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.faults
            .result(Operation::Seek, self.inner.seek(position))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::StorageFull, "private path"))
        }
    }
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::QuotaExceeded, "private URL"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("private token"))
        }
    }
    impl Seek for Broken {
        fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
            Err(io::Error::other("private path"))
        }
    }
    #[test]
    fn captures_each_local_operation_without_messages_and_retains_first_across_reuse() {
        for operation in [
            Operation::Read,
            Operation::Write,
            Operation::Flush,
            Operation::Seek,
        ] {
            let faults = Faults::default();
            let mut handle = Handle {
                inner: Broken,
                faults: faults.clone(),
            };
            match operation {
                Operation::Read => {
                    let _ = handle.read(&mut [0]);
                }
                Operation::Write => {
                    let _ = handle.write(&[0]);
                }
                Operation::Flush => {
                    let _ = handle.flush();
                }
                Operation::Seek => {
                    let _ = handle.seek(SeekFrom::Start(0));
                }
                _ => unreachable!(),
            }
            let first = faults.get().unwrap();
            assert_eq!(first.operation, operation);
            let _ = handle.seek(SeekFrom::Start(1));
            assert_eq!(faults.get(), Some(first));
            let encoded = serde_json::to_string(&first).unwrap();
            assert!(!encoded.contains("private"));
            assert_eq!(Faults::default().get(), None);
        }
    }
    #[test]
    fn create_fault_and_normal_eof_are_distinct() {
        let faults = Faults::default();
        struct FailCreate;
        impl StorageProvider for FailCreate {
            type Reader = io::Cursor<Vec<u8>>;
            type Writer = io::Cursor<Vec<u8>>;
            fn into_reader_writer(
                self,
                _: Option<u64>,
            ) -> io::Result<(Self::Reader, Self::Writer)> {
                Err(io::Error::new(io::ErrorKind::QuotaExceeded, "secret"))
            }
        }
        assert!(Observed {
            inner: FailCreate,
            faults: faults.clone()
        }
        .into_reader_writer(None)
        .is_err());
        assert_eq!(
            faults.get(),
            Some(Fault {
                operation: Operation::Create,
                kind: Kind::QuotaExceeded
            })
        );
        let faults = Faults::default();
        let mut reader = Handle {
            inner: io::Cursor::new(Vec::<u8>::new()),
            faults: faults.clone(),
        };
        assert_eq!(reader.read(&mut [0]).unwrap(), 0);
        assert_eq!(faults.get(), None);
        faults.record(
            Operation::Read,
            &io::Error::from(io::ErrorKind::Interrupted),
        );
        assert_eq!(faults.get(), None);
    }
}
