//! Go's `sort.SliceStable` (sort/zsortfunc.go: insertion sort in blocks of 20, then SymMerge), step for step. For a
//! strict weak order any stable sort gives the same result; a `less` that is not one (cryptotools ranks key lengths
//! whose index of coincidence is within 0.004 of each other as equal) gives Go's answer only with Go's algorithm, and
//! Rust's own sorts may panic on it.

/// Go's `sort.SliceStable(v, less)`.
pub fn slice_stable<T>(v: &mut [T], less: impl Fn(&T, &T) -> bool) {
    let n = v.len();
    let mut block = 20;
    let (mut a, mut b) = (0, block);
    while b <= n {
        insertion(v, a, b, &less);
        a = b;
        b += block;
    }
    insertion(v, a, n, &less);
    while block < n {
        a = 0;
        b = 2 * block;
        while b <= n {
            sym_merge(v, a, a + block, b, &less);
            a = b;
            b += 2 * block;
        }
        let m = a + block;
        if m < n {
            sym_merge(v, a, m, n, &less);
        }
        block *= 2;
    }
}

fn insertion<T>(v: &mut [T], a: usize, b: usize, less: &impl Fn(&T, &T) -> bool) {
    for i in a + 1..b {
        let mut j = i;
        while j > a && less(&v[j], &v[j - 1]) {
            v.swap(j, j - 1);
            j -= 1;
        }
    }
}

fn sym_merge<T>(v: &mut [T], a: usize, m: usize, b: usize, less: &impl Fn(&T, &T) -> bool) {
    if m - a == 1 {
        let (mut i, mut j) = (m, b);
        while i < j {
            let h = (i + j) >> 1;
            if less(&v[h], &v[a]) {
                i = h + 1;
            } else {
                j = h;
            }
        }
        for k in a..i.saturating_sub(1) {
            v.swap(k, k + 1);
        }
        return;
    }
    if b - m == 1 {
        let (mut i, mut j) = (a, m);
        while i < j {
            let h = (i + j) >> 1;
            if !less(&v[m], &v[h]) {
                i = h + 1;
            } else {
                j = h;
            }
        }
        let mut k = m;
        while k > i {
            v.swap(k, k - 1);
            k -= 1;
        }
        return;
    }
    let mid = (a + b) >> 1;
    let n = mid + m;
    let (mut start, mut r) = if m > mid { (n - b, mid) } else { (a, m) };
    let p = n - 1;
    while start < r {
        let c = (start + r) >> 1;
        if !less(&v[p - c], &v[c]) {
            start = c + 1;
        } else {
            r = c;
        }
    }
    let end = n - start;
    if start < m && m < end {
        rotate(v, start, m, end);
    }
    if a < start && start < mid {
        sym_merge(v, a, start, mid, less);
    }
    if mid < end && end < b {
        sym_merge(v, mid, end, b, less);
    }
}

fn rotate<T>(v: &mut [T], a: usize, m: usize, b: usize) {
    let (mut i, mut j) = (m - a, b - m);
    while i != j {
        if i > j {
            swap_range(v, m - i, m, j);
            i -= j;
        } else {
            swap_range(v, m - i, m + j - i, i);
            j -= i;
        }
    }
    swap_range(v, m - i, m, i);
}

fn swap_range<T>(v: &mut [T], a: usize, b: usize, n: usize) {
    for i in 0..n {
        v.swap(a + i, b + i);
    }
}
