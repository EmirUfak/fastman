pub const MAX_INTERPOLATED_BYTES: usize = 4 * 1024 * 1024;

/// Resolve each token once. `name` is absent for an unclosed token.
pub(crate) fn expand<'a, E>(
    input: &'a str,
    mut resolve: impl FnMut(Option<&'a str>, &'a str) -> Result<&'a str, E>,
    too_large: impl Fn() -> E,
) -> Result<String, E> {
    let mut output = String::with_capacity(input.len().min(MAX_INTERPOLATED_BYTES));
    let mut push = |piece: &str| {
        if piece.len() > MAX_INTERPOLATED_BYTES - output.len() {
            return Err(too_large());
        }
        output.push_str(piece);
        Ok(())
    };
    let mut remainder = input;
    while let Some(start) = remainder.find("{{") {
        push(&remainder[..start])?;
        let raw = &remainder[start..];
        let after_open = &raw[2..];
        let Some(end) = after_open.find("}}") else {
            push(resolve(None, raw)?)?;
            return Ok(output);
        };
        let (token, rest) = raw.split_at(end + 4);
        push(resolve(Some(after_open[..end].trim()), token)?)?;
        remainder = rest;
    }
    push(remainder)?;
    Ok(output)
}
