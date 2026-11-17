//! `args` — reading a tool's arguments without each tool inventing its own way.
//!
//! A model sends whatever it sends: `"7"`, `7`, `7.0`, `{}` or a field it
//! invented. The two rules here are that an absent argument takes a default
//! silently (that is normal, the schema says it is optional) and a **present but
//! unparseable one is an error the model can read**, because the alternative is
//! answering the question with a default the caller did not ask for and nobody
//! ever noticing. Every accessor clamps to a stated range rather than trusting a
//! number that could make a tool fetch ten thousand rows.

use common_json::Json;

/// A string argument, trimmed; empty or non-string counts as absent.
pub fn str_arg(args: &Json, name: &str) -> Option<String> {
    args.get(name)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// An integer argument with a hard range, or `None` if absent.
pub fn int_arg(args: &Json, name: &str, min: i64, max: i64) -> Result<Option<i64>, String> {
    let Some(v) = args.get(name) else {
        return Ok(None);
    };
    // Accept a JSON number or a numeric string: both are common from small models.
    let raw = match v {
        Json::Num(n) => Some(*n),
        Json::Str(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    };
    let n = raw.ok_or_else(|| format!("argument \"{name}\" must be a whole number between {min} and {max}"))?;
    if !n.is_finite() {
        return Err(format!("argument \"{name}\" is not a number"));
    }
    let n = n.round() as i64;
    if n < min || n > max {
        return Err(format!("argument \"{name}\" must be between {min} and {max}, got {n}"));
    }
    Ok(Some(n))
}

/// An integer argument with a default already applied.
pub fn int_arg_default(args: &Json, name: &str, default: i64, min: i64, max: i64) -> Result<i64, String> {
    Ok(int_arg(args, name, min, max)?.unwrap_or(default))
}

/// A required string, with the message the schema promised.
pub fn required_str(args: &Json, name: &str, hint: &str) -> Result<String, String> {
    str_arg(args, name).ok_or_else(|| format!("argument \"{name}\" is required ({hint})"))
}

/// A number argument — not rounded, unlike [`int_arg`], because a quantity that has been
/// rounded is a different quantity. Accepts a JSON number or a numeric string for the same
/// reason `int_arg` does.
pub fn num_arg(args: &Json, name: &str) -> Result<Option<f64>, String> {
    let Some(v) = args.get(name) else {
        return Ok(None);
    };
    let raw = match v {
        Json::Num(n) => Some(*n),
        Json::Str(s) => s.trim().replace(',', "").parse::<f64>().ok(),
        _ => None,
    };
    let n = raw.ok_or_else(|| format!("argument \"{name}\" must be a number, e.g. 3.75"))?;
    if !n.is_finite() {
        return Err(format!("argument \"{name}\" is not a number"));
    }
    Ok(Some(n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use common_json as json;

    fn with(pairs: Vec<(&str, Json)>) -> Json {
        json::object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<Vec<_>>())
    }

    #[test]
    fn a_number_and_a_numeric_string_are_the_same_argument() {
        assert_eq!(int_arg(&with(vec![("d", json::number_from_f64(7.0))]), "d", 1, 60).unwrap(), Some(7));
        assert_eq!(int_arg(&with(vec![("d", json::string("7"))]), "d", 1, 60).unwrap(), Some(7));
        assert_eq!(int_arg(&with(vec![("d", json::number_from_f64(6.6))]), "d", 1, 60).unwrap(), Some(7));
    }

    #[test]
    fn absent_is_silent_and_present_but_wrong_is_not() {
        assert_eq!(int_arg(&with(vec![]), "d", 1, 60).unwrap(), None);
        assert_eq!(int_arg_default(&with(vec![]), "d", 7, 1, 60).unwrap(), 7);
        let e = int_arg(&with(vec![("d", json::string("last week"))]), "d", 1, 60).unwrap_err();
        assert!(e.contains("\"d\" must be a whole number"), "{e}");
        let e = int_arg(&with(vec![("d", json::number_from_f64(9999.0))]), "d", 1, 60).unwrap_err();
        assert!(e.contains("between 1 and 60"), "{e}");
    }

    #[test]
    fn an_empty_string_is_the_same_as_no_answer() {
        assert_eq!(str_arg(&with(vec![("q", json::string("   "))]), "q"), None);
        assert_eq!(str_arg(&with(vec![("q", json::string(" workshop "))]), "q").as_deref(), Some("workshop"));
        let e = required_str(&with(vec![]), "entity_id", "an entity id").unwrap_err();
        assert!(e.contains("required"), "{e}");
    }

    #[test]
    fn a_number_argument_keeps_its_fraction_and_loses_its_thousands_separator() {
        assert_eq!(num_arg(&with(vec![("n", json::number_from_f64(3.75))]), "n").unwrap(), Some(3.75));
        assert_eq!(num_arg(&with(vec![("n", json::string("3.75"))]), "n").unwrap(), Some(3.75));
        // Models write "1,000" for a thousand, and a comma is not a decimal point here.
        assert_eq!(num_arg(&with(vec![("n", json::string("1,000"))]), "n").unwrap(), Some(1000.0));
        assert_eq!(num_arg(&with(vec![]), "n").unwrap(), None);
        let e = num_arg(&with(vec![("n", json::string("three quarters"))]), "n").unwrap_err();
        assert!(e.contains("must be a number"), "{e}");
    }
}
