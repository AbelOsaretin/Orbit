// Minimal in-memory stand-in for the Supabase query builder. Rows are stored as
// plain objects; embedded relations (e.g. `plans` on a subscription) are stored
// nested on the row so dotted filters like `plans.merchant_id` work.
const get = (row, path) => path.split('.').reduce((value, key) => value?.[key], row);

export function createFakeSupabase(seed = {}, { unique = {} } = {}) {
    const tables = Object.fromEntries(Object.entries(seed).map(([name, rows]) => [name, rows.map((r) => ({ ...r }))]));
    const rowsOf = (table) => (tables[table] ||= []);

    const violatesUnique = (table, candidate, ignore) => (unique[table] || []).some((cols) =>
        rowsOf(table).some((row) => row !== ignore && cols.every((c) => row[c] !== undefined && row[c] === candidate[c])));

    function from(table) {
        const filters = [];
        let op = 'select';
        let payload = null;
        let upsertConflict = null;
        let limit = null;

        const matches = (row) => filters.every((f) => f(row));
        const execute = () => {
            const rows = rowsOf(table);
            if (op === 'insert') {
                for (const row of payload) {
                    if (violatesUnique(table, row)) return { data: null, error: { code: '23505', message: 'duplicate key value' } };
                }
                const inserted = payload.map((row) => ({ ...row }));
                rows.push(...inserted);
                return { data: inserted, error: null };
            }
            if (op === 'upsert') {
                const out = [];
                for (const row of payload) {
                    const existing = rows.find((r) => upsertConflict.every((c) => r[c] === row[c]));
                    if (existing) Object.assign(existing, row);
                    else rows.push({ ...row });
                    out.push(existing || rows[rows.length - 1]);
                }
                return { data: out, error: null };
            }
            const selected = rows.filter(matches);
            if (op === 'update') {
                for (const row of selected) Object.assign(row, payload);
                return { data: selected, error: null };
            }
            if (op === 'delete') {
                tables[table] = rows.filter((row) => !selected.includes(row));
                return { data: selected, error: null };
            }
            return { data: limit === null ? selected : selected.slice(0, limit), error: null };
        };

        const builder = {
            select: () => builder,
            insert: (rows) => { op = 'insert'; payload = [].concat(rows); return builder; },
            upsert: (rows, { onConflict = 'id' } = {}) => {
                op = 'upsert'; payload = [].concat(rows); upsertConflict = onConflict.split(',').map((c) => c.trim());
                return builder;
            },
            update: (patch) => { op = 'update'; payload = patch; return builder; },
            delete: () => { op = 'delete'; return builder; },
            eq: (path, value) => { filters.push((row) => get(row, path) === value); return builder; },
            neq: (path, value) => { filters.push((row) => get(row, path) !== value); return builder; },
            gte: (path, value) => { filters.push((row) => get(row, path) >= value); return builder; },
            lte: (path, value) => { filters.push((row) => get(row, path) <= value); return builder; },
            in: (path, values) => { filters.push((row) => values.includes(get(row, path))); return builder; },
            is: (path, value) => { filters.push((row) => (get(row, path) ?? null) === value); return builder; },
            order: () => builder,
            limit: (n) => { limit = n; return builder; },
            maybeSingle: async () => {
                const { data, error } = execute();
                if (error) return { data: null, error };
                return { data: data[0] ?? null, error: null };
            },
            single: async () => {
                const { data, error } = execute();
                if (error) return { data: null, error };
                if (!data.length) return { data: null, error: { code: 'PGRST116', message: 'no rows' } };
                return { data: data[0], error: null };
            },
            then: (resolve, reject) => Promise.resolve().then(execute).then(resolve, reject),
        };
        return builder;
    }

    return {
        tables,
        from,
        rpc: async () => ({ data: null, error: null }),
    };
}
