//! Effect building block. Owned by the effects work stream.
//! Realtime rules: no allocation, locking or I/O after `prepare`.
