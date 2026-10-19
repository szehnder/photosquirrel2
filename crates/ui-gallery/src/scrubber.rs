//! Timeline scrubber (ADR-0018): year and month marks of a date-sorted list, found by binary search over
//! the sort keys (no pass over the items): O(years x 12 x log n).

use ps_model::{Sort, SortField, SortKey};
use ps_model::{civil_from_days, days_from_civil};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub year: i32,
    /// `None` for the year mark itself.
    pub month: Option<u8>,
    /// First list index of the period.
    pub index: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Timeline {
    /// Year marks with their months, in list order.
    pub marks: Vec<Mark>,
    /// First undated item (capture-date sorts put them last), if any.
    pub undated: Option<usize>,
    pub count: usize,
}

const DAY_MS: i64 = 86_400_000;

fn year_start_ms(year: i32, month: u8) -> i64 {
    days_from_civil(year, month, 1) * DAY_MS
}

fn year_of_ms(ms: i64) -> (i32, u8) {
    let (y, m, _) = civil_from_days(ms.div_euclid(DAY_MS));
    (y, m)
}

impl Timeline {
    /// Marks for `keys` sorted by `sort`; empty unless the sort is by capture date.
    pub fn build(keys: &[SortKey], sort: Sort) -> Timeline {
        let mut t = Timeline {
            count: keys.len(),
            ..Timeline::default()
        };
        if sort.field != SortField::CaptureDate || keys.is_empty() {
            return t;
        }
        let undated_at = keys.partition_point(|k| k.primary() != SortKey::UNDATED);
        if undated_at < keys.len() {
            t.undated = Some(undated_at);
        }
        if undated_at == 0 {
            return t;
        }
        let dated = &keys[..undated_at];
        let ms_of = |k: &SortKey| {
            let p = if sort.descending {
                !k.primary()
            } else {
                k.primary()
            };
            SortKey::primary_signed(p)
        };
        let (a, b) = (ms_of(&dated[0]), ms_of(&dated[dated.len() - 1]));
        let (first, last) = if a <= b { (a, b) } else { (b, a) };
        let (y0, _) = year_of_ms(first);
        let (y1, _) = year_of_ms(last);
        // First index whose capture is >= `ms` (ascending) or < `ms` (descending, the period's latest
        // item comes first).
        let index_of = |start_ms: i64, end_ms: i64| -> usize {
            if sort.descending {
                let bound = !SortKey::signed_primary(end_ms);
                dated.partition_point(|k| k.primary() <= bound)
            } else {
                let bound = SortKey::signed_primary(start_ms);
                dated.partition_point(|k| k.primary() < bound)
            }
        };
        let years: Vec<i32> = if sort.descending {
            (y0..=y1).rev().collect()
        } else {
            (y0..=y1).collect()
        };
        for year in years {
            let months: Vec<u8> = if sort.descending {
                (1..=12).rev().collect()
            } else {
                (1..=12).collect()
            };
            let mut year_mark = None;
            let mut month_marks = Vec::new();
            for month in months {
                let start = year_start_ms(year, month);
                let end = if month == 12 {
                    year_start_ms(year + 1, 1)
                } else {
                    year_start_ms(year, month + 1)
                };
                let i = index_of(start, end);
                // Non-empty month: its first item is inside it.
                if i < dated.len() {
                    let ms = ms_of(&dated[i]);
                    if ms >= start && ms < end {
                        year_mark.get_or_insert(i);
                        month_marks.push(Mark {
                            year,
                            month: Some(month),
                            index: i,
                        });
                    }
                }
            }
            if let Some(i) = year_mark {
                t.marks.push(Mark {
                    year,
                    month: None,
                    index: i,
                });
                t.marks.extend(month_marks);
            }
        }
        t
    }

    pub fn years(&self) -> impl Iterator<Item = &Mark> {
        self.marks.iter().filter(|m| m.month.is_none())
    }

    /// The first item of `year` (and month), if present.
    pub fn index_of(&self, year: i32, month: Option<u8>) -> Option<usize> {
        self.marks
            .iter()
            .find(|m| m.year == year && m.month == month)
            .map(|m| m.index)
    }

    /// The year an index falls in.
    pub fn year_at(&self, index: usize) -> Option<i32> {
        if self.undated.is_some_and(|u| index >= u) {
            return None;
        }
        self.years()
            .take_while(|m| m.index <= index)
            .last()
            .map(|m| m.year)
    }

    pub fn is_empty(&self) -> bool {
        self.marks.is_empty() && self.undated.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ps_model::ItemId;

    fn keys(dates: &[(i32, u8, u8)], descending: bool) -> Vec<SortKey> {
        let mut ms: Vec<i64> = dates
            .iter()
            .map(|(y, m, d)| days_from_civil(*y, *m, *d) * DAY_MS)
            .collect();
        ms.sort();
        if descending {
            ms.reverse();
        }
        ms.iter()
            .enumerate()
            .map(|(i, v)| {
                let p = SortKey::signed_primary(*v);
                SortKey::new(if descending { !p } else { p }, ItemId(i as u32), 0, 0)
            })
            .collect()
    }

    #[test]
    fn years_and_months_ascending_and_descending() {
        let dates = [
            (2010, 3, 1),
            (2010, 3, 9),
            (2012, 1, 5),
            (2012, 7, 1),
            (2014, 12, 31),
        ];
        let asc = Timeline::build(&keys(&dates, false), Sort::default());
        let years: Vec<(i32, usize)> = asc.years().map(|m| (m.year, m.index)).collect();
        assert_eq!(years, vec![(2010, 0), (2012, 2), (2014, 4)]);
        assert_eq!(asc.index_of(2012, Some(7)), Some(3));
        assert_eq!(asc.year_at(3), Some(2012));
        let desc = Timeline::build(
            &keys(&dates, true),
            Sort {
                descending: true,
                ..Sort::default()
            },
        );
        let years: Vec<(i32, usize)> = desc.years().map(|m| (m.year, m.index)).collect();
        assert_eq!(years, vec![(2014, 0), (2012, 1), (2010, 3)]);
    }
}
