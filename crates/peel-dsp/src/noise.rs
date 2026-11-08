//! Instrument building block. Owned by the instruments work stream.
//! Realtime rules: no allocation, locking or I/O after `prepare`.
