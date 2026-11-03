//! Local wall-clock timestamps for file names, without a date crate. These replace the
//! `date +%H-%M-%S` and `date +%Y-%m-%d_%H-%M-%S` calls in the bash scripts.

fn local_tm() -> Option<libc::tm> {
    // SAFETY: `time` with a null pointer only returns the value; `localtime_r` writes into the
    // zeroed `tm` we own and returns null on failure, which we check.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            None
        } else {
            Some(tm)
        }
    }
}

/// `HH-MM-SS`, local time.
pub fn hms() -> String {
    match local_tm() {
        Some(tm) => format!("{:02}-{:02}-{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec),
        None => "00-00-00".into(),
    }
}

/// `YYYY-MM-DD_HH-MM-SS`, local time.
pub fn stamp() -> String {
    match local_tm() {
        Some(tm) => format!(
            "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        ),
        None => "0000-00-00_00-00-00".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hms_has_the_h_m_s_shape() {
        let t = hms();
        let p: Vec<u32> = t.split('-').map(|x| x.parse().unwrap()).collect();
        assert_eq!(t.len(), 8, "{t}");
        assert!(p[0] < 24 && p[1] < 60 && p[2] < 61, "{t}");
    }

    #[test]
    fn stamp_has_the_date_time_shape() {
        let t = stamp();
        assert_eq!(t.len(), 19, "{t}");
        let (date, time) = t.split_once('_').unwrap();
        let d: Vec<u32> = date.split('-').map(|x| x.parse().unwrap()).collect();
        let h: Vec<u32> = time.split('-').map(|x| x.parse().unwrap()).collect();
        assert!(
            d[0] >= 2026 && (1..=12).contains(&d[1]) && (1..=31).contains(&d[2]),
            "{t}"
        );
        assert!(h[0] < 24 && h[1] < 60 && h[2] < 61, "{t}");
        assert!(t.starts_with(&date[..4]));
    }
}
