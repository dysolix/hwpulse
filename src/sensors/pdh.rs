//! Minimal safe wrapper over Windows Performance Data Helper (PDH) — the same
//! counters Task Manager and perfmon read. Uses English counter paths so it works
//! on localized Windows installs.

use windows::Win32::System::Performance::{
    PDH_FMT, PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
    PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows::core::{HSTRING, PCWSTR};

/// Don't clamp percentages at 100 (e.g. "% Processor Performance" during boost).
const PDH_FMT_NOCAP100: u32 = 0x8000;
const FORMAT: PDH_FMT = PDH_FMT(PDH_FMT_DOUBLE.0 | PDH_FMT_NOCAP100);
const ERROR_SUCCESS: u32 = 0;

pub struct Query(PDH_HQUERY);

#[derive(Clone, Copy)]
pub struct Counter(PDH_HCOUNTER);

impl Query {
    pub fn new() -> Option<Self> {
        let mut handle = PDH_HQUERY::default();
        let status = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut handle) };
        (status == ERROR_SUCCESS).then_some(Self(handle))
    }

    /// Adds a counter such as `\Processor Information(_Total)\% Processor Utility`.
    /// Wildcard instances (`(*)`) are read with [`Query::array`].
    pub fn add(&self, path: &str) -> Option<Counter> {
        let mut counter = PDH_HCOUNTER::default();
        let status = unsafe { PdhAddEnglishCounterW(self.0, &HSTRING::from(path), 0, &mut counter) };
        (status == ERROR_SUCCESS).then_some(Counter(counter))
    }

    /// Samples all counters. Rate counters need two samples before they report values.
    pub fn collect(&self) {
        unsafe { PdhCollectQueryData(self.0) };
    }

    pub fn value(&self, counter: Counter) -> Option<f64> {
        let mut value = PDH_FMT_COUNTERVALUE::default();
        let status = unsafe { PdhGetFormattedCounterValue(counter.0, FORMAT, None, &mut value) };
        (status == ERROR_SUCCESS && value.CStatus == ERROR_SUCCESS).then_some(unsafe { value.Anonymous.doubleValue })
    }

    /// All instances of a wildcard counter as `(instance name, value)`.
    pub fn array(&self, counter: Counter) -> Vec<(String, f64)> {
        let (mut bytes, mut count) = (0u32, 0u32);
        let status = unsafe { PdhGetFormattedCounterArrayW(counter.0, FORMAT, &mut bytes, &mut count, None) };
        if status != PDH_MORE_DATA || bytes == 0 {
            return Vec::new();
        }

        // The buffer holds the item structs followed by their name strings, so it is
        // sized in bytes, but allocated as items to keep the alignment right.
        let item_size = size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        let mut buf = vec![PDH_FMT_COUNTERVALUE_ITEM_W::default(); (bytes as usize).div_ceil(item_size)];
        let status =
            unsafe { PdhGetFormattedCounterArrayW(counter.0, FORMAT, &mut bytes, &mut count, Some(buf.as_mut_ptr())) };
        if status != ERROR_SUCCESS {
            return Vec::new();
        }

        buf[..count as usize]
            .iter()
            .filter(|item| item.FmtValue.CStatus == ERROR_SUCCESS)
            .map(|item| {
                let name = unsafe { item.szName.to_string() }.unwrap_or_default();
                (name, unsafe { item.FmtValue.Anonymous.doubleValue })
            })
            .collect()
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.0) };
    }
}
