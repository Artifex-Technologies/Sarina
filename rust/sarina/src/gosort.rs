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

/// Go's `sort.Slice(v, less)`: pattern-defeating quicksort (sort/zsortfunc.go's `pdqsort_func`), step for step, so that
/// elements `less` calls equal end in Go's order.
pub fn slice<T>(v: &mut [T], less: impl Fn(&T, &T) -> bool) {
    let n = v.len() as isize;
    let limit = (usize::BITS - v.len().leading_zeros()) as isize;
    pdq(v, 0, n, limit, &less);
}

fn pdq<T>(v: &mut [T], mut a: isize, mut b: isize, mut limit: isize, less: &impl Fn(&T, &T) -> bool) {
    let lt = |v: &[T], i: isize, j: isize| less(&v[i as usize], &v[j as usize]);
    let (mut was_balanced, mut was_partitioned) = (true, true);
    loop {
        let length = b - a;
        if length <= 12 {
            insertion(v, a as usize, b as usize, less);
            return;
        }
        if limit == 0 {
            heap_sort(v, a, b, less);
            return;
        }
        if !was_balanced {
            break_patterns(v, a, b);
            limit -= 1;
        }
        let (mut pivot, mut hint) = choose_pivot(v, a, b, less);
        if hint == Hint::Decreasing {
            reverse_range(v, a, b);
            pivot = (b - 1) - (pivot - a);
            hint = Hint::Increasing;
        }
        if was_balanced && was_partitioned && hint == Hint::Increasing && partial_insertion(v, a, b, less) {
            return;
        }
        if a > 0 && !lt(v, a - 1, pivot) {
            a = partition_equal(v, a, b, pivot, less);
            continue;
        }
        let (mid, already) = partition(v, a, b, pivot, less);
        was_partitioned = already;
        let (left, right) = (mid - a, b - mid);
        let threshold = length / 8;
        if left < right {
            was_balanced = left >= threshold;
            pdq(v, a, mid, limit, less);
            a = mid + 1;
        } else {
            was_balanced = right >= threshold;
            pdq(v, mid + 1, b, limit, less);
            b = mid;
        }
    }
}

fn swap<T>(v: &mut [T], i: isize, j: isize) {
    v.swap(i as usize, j as usize);
}

fn sift_down<T>(v: &mut [T], lo: isize, hi: isize, first: isize, less: &impl Fn(&T, &T) -> bool) {
    let lt = |v: &[T], i: isize, j: isize| less(&v[i as usize], &v[j as usize]);
    let mut root = lo;
    loop {
        let mut child = 2 * root + 1;
        if child >= hi {
            return;
        }
        if child + 1 < hi && lt(v, first + child, first + child + 1) {
            child += 1;
        }
        if !lt(v, first + root, first + child) {
            return;
        }
        swap(v, first + root, first + child);
        root = child;
    }
}

fn heap_sort<T>(v: &mut [T], a: isize, b: isize, less: &impl Fn(&T, &T) -> bool) {
    let hi = b - a;
    let mut i = (hi - 1) / 2;
    while i >= 0 {
        sift_down(v, i, hi, a, less);
        i -= 1;
    }
    let mut i = hi - 1;
    while i >= 0 {
        swap(v, a, a + i);
        sift_down(v, 0, i, a, less);
        i -= 1;
    }
}

fn partition<T>(v: &mut [T], a: isize, b: isize, pivot: isize, less: &impl Fn(&T, &T) -> bool) -> (isize, bool) {
    let lt = |v: &[T], i: isize, j: isize| less(&v[i as usize], &v[j as usize]);
    swap(v, a, pivot);
    let (mut i, mut j) = (a + 1, b - 1);
    while i <= j && lt(v, i, a) {
        i += 1;
    }
    while i <= j && !lt(v, j, a) {
        j -= 1;
    }
    if i > j {
        swap(v, j, a);
        return (j, true);
    }
    swap(v, i, j);
    i += 1;
    j -= 1;
    loop {
        while i <= j && lt(v, i, a) {
            i += 1;
        }
        while i <= j && !lt(v, j, a) {
            j -= 1;
        }
        if i > j {
            break;
        }
        swap(v, i, j);
        i += 1;
        j -= 1;
    }
    swap(v, j, a);
    (j, false)
}

fn partition_equal<T>(v: &mut [T], a: isize, b: isize, pivot: isize, less: &impl Fn(&T, &T) -> bool) -> isize {
    let lt = |v: &[T], i: isize, j: isize| less(&v[i as usize], &v[j as usize]);
    swap(v, a, pivot);
    let (mut i, mut j) = (a + 1, b - 1);
    loop {
        while i <= j && !lt(v, a, i) {
            i += 1;
        }
        while i <= j && lt(v, a, j) {
            j -= 1;
        }
        if i > j {
            return i;
        }
        swap(v, i, j);
        i += 1;
        j -= 1;
    }
}

fn partial_insertion<T>(v: &mut [T], a: isize, b: isize, less: &impl Fn(&T, &T) -> bool) -> bool {
    let lt = |v: &[T], i: isize, j: isize| less(&v[i as usize], &v[j as usize]);
    let mut i = a + 1;
    for _ in 0..5 {
        while i < b && !lt(v, i, i - 1) {
            i += 1;
        }
        if i == b {
            return true;
        }
        if b - a < 50 {
            return false;
        }
        swap(v, i, i - 1);
        if i - a >= 2 {
            // Go's loop runs down to index 1, not to a
            let mut j = i - 1;
            while j >= 1 && lt(v, j, j - 1) {
                swap(v, j, j - 1);
                j -= 1;
            }
        }
        if b - i >= 2 {
            let mut j = i + 1;
            while j < b && lt(v, j, j - 1) {
                swap(v, j, j - 1);
                j += 1;
            }
        }
    }
    false
}

fn break_patterns<T>(v: &mut [T], a: isize, b: isize) {
    let length = b - a;
    if length >= 8 {
        let mut random = length as u64;
        let modulus = 1u64 << (u64::BITS - (length as u64).leading_zeros());
        let mut idx = a + (length / 4) * 2 - 1;
        while idx <= a + (length / 4) * 2 + 1 {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            let mut other = (random & (modulus - 1)) as isize;
            if other >= length {
                other -= length;
            }
            swap(v, idx, a + other);
            idx += 1;
        }
    }
}

#[derive(PartialEq)]
enum Hint {
    Unknown,
    Increasing,
    Decreasing,
}

fn choose_pivot<T>(v: &[T], a: isize, b: isize, less: &impl Fn(&T, &T) -> bool) -> (isize, Hint) {
    let l = b - a;
    let mut swaps = 0;
    let (mut i, mut j, mut k) = (a + l / 4, a + l / 4 * 2, a + l / 4 * 3);
    if l >= 8 {
        if l >= 50 {
            i = median(v, i - 1, i, i + 1, &mut swaps, less);
            j = median(v, j - 1, j, j + 1, &mut swaps, less);
            k = median(v, k - 1, k, k + 1, &mut swaps, less);
        }
        j = median(v, i, j, k, &mut swaps, less);
    }
    match swaps {
        0 => (j, Hint::Increasing),
        12 => (j, Hint::Decreasing),
        _ => (j, Hint::Unknown),
    }
}

fn median<T>(v: &[T], a: isize, b: isize, c: isize, swaps: &mut i32, less: &impl Fn(&T, &T) -> bool) -> isize {
    let mut order2 = |x: isize, y: isize| {
        if less(&v[y as usize], &v[x as usize]) {
            *swaps += 1;
            (y, x)
        } else {
            (x, y)
        }
    };
    let (a, b) = order2(a, b);
    let (b, _) = order2(b, c);
    let (_, b) = order2(a, b);
    b
}

fn reverse_range<T>(v: &mut [T], a: isize, b: isize) {
    v[a as usize..b as usize].reverse();
}
