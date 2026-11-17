//! `tools_basics` — the three things a person expects an assistant to be able to do that
//! this network had no way to ask for.
//!
//! They live **here** rather than in a bolted-on third-party MCP server for one reason: this
//! process is the one place every harness already reaches (Slack, voice, the OpenAI-compat
//! endpoint), and `docs/slack-agent-2026-11-16.md`'s whole argument is that a fact must come
//! from one tool so a voice answer and a Slack answer cannot disagree. A clock that agrees
//! with itself is the entire point.
//!
//! The second reason is that these three are not "demo tools". Each replaces something the
//! model is currently asked to do in its head:
//!
//! * **the time** — `prompt.rs` tells the model "the space is in the UK (UTC in winter, UTC+1
//!   in summer)", which is an instruction to do daylight-saving arithmetic from memory,
//!   silently, on a quantised coder model, in a room where people time things. `clock.rs`'s
//!   own header calls that class of bug the kind that "survives forever because the answer is
//!   plausible either way". Now there is a tool, and the answer carries its zone label.
//! * **arithmetic** — same argument. `3/8` is a division the model may or may not perform,
//!   and a wrong resistor value is a component released into the smoke.
//! * **units** — a lookup, so the model cannot invent that an inch is 2.5 cm.
//!
//! All three are **pure**: no HTTP, no files, no secrets, no shell. `tools::register`'s
//! property — a tool can reach nothing but the `Sources` it was handed — still holds here in
//! the strong sense, since these three take `&Sources` and ignore it.

use crate::args;
use crate::calc::{eval, fmt};
use crate::clock;
use crate::sources::Sources;
use crate::tools::register;
use common_json::Json;
use common_mcp::{object_schema, param, Server};
use std::sync::Arc;

pub fn register_tools(srv: &mut Server, src: &Arc<Sources>) {
    register(
        srv,
        "what_time",
        "The hackspace's clock: today's date, the weekday, and the time right now in \
         Europe/London with its zone label (GMT or BST, and the UTC offset), plus the same \
         moment in UTC. Use this for \"what time is it\", \"what day is it\", \"is it the \
         weekend\", \"has the clock changed\". \
         NEVER answer a time from your own arithmetic: other tools print UTC, and converting \
         it yourself is exactly how a bot ends up an hour wrong for half the year. If you \
         need \"now\" for anything, call this.",
        object_schema(Vec::new(), Vec::new()),
        src,
        what_time,
    );

    register(
        srv,
        "calculate",
        "Do arithmetic exactly. Use it for anything with a number in it rather than working \
         it out yourself — including the workshop cases: \"3/8 of an inch in mm\" (do the \
         fraction here, then convert), watts/volts/amps (P = V × I), series and parallel \
         resistors, resistor dividers, board diagonals (hypot), drop percentages, and \"how \
         many of these fit in that\". \
         Operators: + - * / ^ ( ) and the words mod and sqrt, abs, round, floor, ceil, min, \
         max, pow, hypot, ln, log, exp, sin, cos, tan; pi and e are constants; a trailing % \
         means \"per hundred\" (12% = 0.12), and mod is the remainder. \
         Units are not numbers: strip them (230 volts × 2 amps is just 230*2) and use the \
         convert tool for the units themselves.",
        object_schema(
            vec![(
                "expression",
                param("string", "One arithmetic expression, e.g. \"230*2\", \"3/8*25.4\", \"hypot(600,400)\", \"1/(1/470+1/1000)\"."),
            )],
            vec!["expression"],
        ),
        src,
        calculate,
    );

    register(
        srv,
        "convert",
        "Convert a quantity between units — metric/imperial, temperature, litres and \
         gallons, mph and km/h, kWh and joules, bytes and gigabytes, and so on. Use it for \
         \"what's 3/8 inch in mm\", \"how heavy is 5 lb in kg\", \"20 °C in fahrenheit\", \
         \"how many litres is 3 gallons\". \
         Give amount, from and to. Compute any fraction first with calculate and pass the \
         result as amount. \
         Where an imperial unit differs from its US namesake (gal, floz, pint, quart, ton) \
         the bare name means the **UK** one and the answer says so; add _us for the US unit. \
         With no from, it lists every unit it knows.",
        object_schema(
            vec![
                ("amount", param("number", "The quantity to convert, e.g. 0.375. Compute fractions with calculate first.")),
                ("from", param("string", "Unit to convert from, e.g. in, mm, kg, lb, c, f, gal, mph, kwh.")),
                ("to", param("string", "Unit to convert to, e.g. mm, in, lb, kg, f, c, l, km/h, j.")),
            ],
            Vec::new(),
        ),
        src,
        convert,
    );
}

// ── what_time ──────────────────────────────────────────────────────────────────

fn what_time(_src: &Sources, _arguments: &Json) -> Result<String, String> {
    // The system clock, not a clock of our own: every box here is NTP-synced and a bot with
    // its own idea of the hour is worse than one that admits it cannot see it.
    let now = clock::now_epoch() as i64;
    let off = clock::london_offset(now);
    let local = clock::civil_text(now as f64, off);
    let day = clock::weekday(&local[0..10]).unwrap_or("???");
    let abbr = clock::london_abbr(now);
    let (date, time) = (local[0..10].to_string(), local[11..].to_string());
    Ok(format!(
        "{day} {pretty} {time} {abbr}, which is UTC{:+} — the space's local time in \
         Europe/London. The same moment in UTC is {utc}.",
        off / 3600,
        pretty = clock::nice_date(&date),
        utc = clock::utc_text(now as f64)[11..16].to_string(),
    ))
}

// ── calculate ──────────────────────────────────────────────────────────────────

// The expression parser this tool needs lives in `calc.rs`: it is a grammar, this file is
// a table and a sentence, and they change for different reasons.

fn calculate(_src: &Sources, arguments: &Json) -> Result<String, String> {
    let raw = args::required_str(arguments, "expression", "one arithmetic expression")?;
    if raw.len() > 400 {
        return Err("that expression is too long for me — ask it in smaller pieces".into());
    }
    let v = eval(&raw)?;
    if !v.is_finite() {
        return Err("that came out as not-a-number or infinity".into());
    }
    Ok(format!("{} = {}", raw.trim(), fmt(v)))
}

// ── convert ────────────────────────────────────────────────────────────────────

/// `(display, dimension, factor to the dimension's base unit, aliases, note)`.
///
/// The factors are the defined ones, not measured: 1 in = 25.4 mm exactly, 1 lb =
/// 0.45359237 kg exactly, and the UK gallon is the imperial 4.54609 L. The `note` column is
/// only filled where a bare name is ambiguous between UK and US — and the answer then says
/// which it chose, rather than quietly picking one.
type Unit = (&'static str, &'static str, f64, &'static [&'static str], Option<&'static str>);

const UNITS: &[Unit] = &[
    // length — base: metre
    ("nm", "length", 1e-9, &[], None),
    ("um", "length", 1e-6, &["µm", "micron", "microns"], None),
    ("mm", "length", 0.001, &["millimetre", "millimeter", "millimetres", "millimeters"], None),
    ("cm", "length", 0.01, &["centimetre", "centimeter", "centimetres", "centimeters"], None),
    ("m", "length", 1.0, &["metre", "meter", "metres", "meters"], None),
    ("km", "length", 1000.0, &["kilometre", "kilometer", "kilometres", "kilometers", "kms"], None),
    ("thou", "length", 0.0000254, &["mil", "mils", "thous"], Some("a thou is 1/1000 inch")),
    ("in", "length", 0.0254, &["inch", "inches", "\""], None),
    ("ft", "length", 0.3048, &["foot", "feet", "'"], None),
    ("yd", "length", 0.9144, &["yard", "yards"], None),
    ("mi", "length", 1609.344, &["mile", "miles"], None),
    ("nmi", "length", 1852.0, &["nautical mile", "nauticalmiles", "cable"], None),
    // mass — base: kilogram
    ("mg", "mass", 1e-6, &["milligram", "milligrams"], None),
    ("g", "mass", 0.001, &["gram", "grams", "gramme", "grammes"], None),
    ("kg", "mass", 1.0, &["kilo", "kilos", "kilogram", "kilograms"], None),
    ("t", "mass", 1000.0, &["tonne", "tonnes", "tonnesmetric"], None),
    ("oz", "mass", 0.028349523125, &["ounce", "ounces"], None),
    ("lb", "mass", 0.45359237, &["lbs", "pound", "pounds"], None),
    ("st", "mass", 6.35029318, &["stone", "stones"], Some("a stone is 14 lb")),
    ("ton_us", "mass", 907.18474, &["short ton"], Some("US short ton, 2000 lb")),
    ("ton_uk", "mass", 1016.0469088, &["long ton"], Some("UK long ton, 2240 lb")),
    // volume — base: litre
    ("ml", "volume", 0.001, &["millilitre", "milliliter", "millilitres", "milliliters", "cc", "cmsup3"], None),
    ("cl", "volume", 0.01, &["centilitre", "centiliter"], None),
    ("dl", "volume", 0.1, &["decilitre", "deciliter"], None),
    ("l", "volume", 1.0, &["litre", "liter", "litres", "liters"], None),
    ("m3", "volume", 1000.0, &["cubic metre", "cubic meter", "m³"], None),
    ("cc", "volume", 0.001, &["cm3", "cubic centimetre", "cubic centimeter"], None),
    ("floz_us", "volume", 0.0295735295625, &["fluid ounce us"], Some("US fluid ounce")),
    ("floz", "volume", 0.0284130625, &["fluid ounce", "fluid ounces"], Some("imperial (UK) fluid ounce — floz_us for the US one")),
    ("pint_us", "volume", 0.473176473, &[], Some("US pint")),
    ("pint", "volume", 0.56826125, &["pints", "pt"], Some("imperial (UK) pint — pint_us for the US one")),
    ("quart_us", "volume", 0.946352946, &[], Some("US quart")),
    ("quart", "volume", 1.1365225, &["quarts", "qt"], Some("imperial (UK) quart — quart_us for the US one")),
    ("gal_us", "volume", 3.785411784, &[], Some("US gallon")),
    ("gal", "volume", 4.54609, &["gallon", "gallons"], Some("imperial (UK) gallon — gal_us for the US one")),
    ("ft3", "volume", 28.316846592, &["cubic foot", "cubic feet", "cft"], None),
    ("in3", "volume", 0.016387064, &["cubic inch", "cubic inches"], None),
    // area — base: square metre
    ("mm2", "area", 1e-6, &["sq mm", "square millimetre", "square millimeter"], None),
    ("cm2", "area", 1e-4, &["sq cm", "square centimetre", "square centimeter"], None),
    ("m2", "area", 1.0, &["sq m", "square metre", "square meter", "sqm", "m²"], None),
    ("ha", "area", 10000.0, &["hectare", "hectares"], None),
    ("acre", "area", 4046.8564224, &["acres"], None),
    ("in2", "area", 0.00064516, &["sq in", "square inch", "square inches"], None),
    ("ft2", "area", 0.09290304, &["sq ft", "square foot", "square feet"], None),
    // speed — base: metre per second
    ("m/s", "speed", 1.0, &["mps", "metres per second", "meters per second"], None),
    ("km/h", "speed", 1.0 / 3.6, &["kph", "kmh", "km per h", "kilometres per hour", "kilometers per hour"], None),
    ("mph", "speed", 0.44704, &["miles per hour", "mile per hour"], None),
    ("kn", "speed", 1852.0 / 3600.0, &["knot", "knots"], None),
    ("ft/s", "speed", 0.3048, &["fps", "feet per second"], None),
    // pressure — base: pascal
    ("pa", "pressure", 1.0, &["pascal", "pascals"], None),
    ("kpa", "pressure", 1000.0, &["kilopascal", "kilopascals"], None),
    ("mpa", "pressure", 1e6, &["megapascal", "megapascals"], None),
    ("bar", "pressure", 100000.0, &["bars"], None),
    ("mbar", "pressure", 100.0, &["millibar", "millibars", "hpa"], None),
    ("psi", "pressure", 6894.757293168, &[], None),
    ("atm", "pressure", 101325.0, &["atmosphere", "atmospheres"], None),
    // energy — base: joule
    ("j", "energy", 1.0, &["joule", "joules"], None),
    ("kj", "energy", 1000.0, &["kilojoule", "kilojoules"], None),
    ("cal", "energy", 4.184, &["calorie", "calories"], None),
    ("kcal", "energy", 4184.0, &["kilocalorie", "kilocalories"], None),
    ("wh", "energy", 3600.0, &["watt hour", "watt hours"], None),
    ("kwh", "energy", 3.6e6, &["kilowatt hour", "kilowatt hours", "units"], None),
    ("mwh", "energy", 3.6e9, &["megawatt hour"], None),
    ("btu", "energy", 1055.05585262, &["btus"], None),
    // power — base: watt
    ("w", "power", 1.0, &["watt", "watts"], None),
    ("kw", "power", 1000.0, &["kilowatt", "kilowatts"], None),
    (
        "mw",
        "power",
        0.001,
        &["milliwatt", "milliwatts"],
        Some("mW is **milli**watts here — for a megawatt write megawatt"),
    ),
    ("megawatt", "power", 1e6, &["megawatts"], None),
    ("hp", "power", 745.6998715822702, &["horsepower"], Some("mechanical horsepower")),
    // time — base: second
    ("ms", "time", 0.001, &["millisecond", "milliseconds"], None),
    ("s", "time", 1.0, &["second", "seconds", "sec"], None),
    ("min", "time", 60.0, &["minute", "minutes"], None),
    ("h", "time", 3600.0, &["hour", "hours", "hr", "hrs"], None),
    ("day", "time", 86400.0, &["days", "d"], None),
    ("week", "time", 604800.0, &["weeks", "wk"], None),
    // data — base: byte
    ("b", "data", 1.0, &["byte", "bytes"], None),
    ("kb", "data", 1000.0, &["kilobyte", "kilobytes"], None),
    ("mb", "data", 1e6, &["megabyte", "megabytes"], None),
    ("gb", "data", 1e9, &["gigabyte", "gigabytes"], None),
    ("tb", "data", 1e12, &["terabyte", "terabytes"], None),
    ("kib", "data", 1024.0, &["kibibyte"], None),
    ("mib", "data", 1_048_576.0, &["mebibyte"], None),
    ("gib", "data", 1_073_741_824.0, &["gibibyte"], None),
    ("bit", "data", 0.125, &["bits"], None),
    // angle — base: degree
    ("deg", "angle", 1.0, &["degree", "degrees", "°", "º"], None),
    ("rad", "angle", 180.0 / std::f64::consts::PI, &["radian", "radians"], None),
    // temperature — the dimension that is not a factor; see `to_kelvin`.
    ("c", "temperature", 1.0, &["celsius", "centigrade", "°c", "degc"], None),
    ("f", "temperature", 1.0, &["fahrenheit", "°f", "degf"], None),
    ("k", "temperature", 1.0, &["kelvin"], None),
    // electrical — one real source of workshop questions that are *not* conversions, so they
    // are named in the error rather than faked here: see `not_a_conversion`.
];

/// The names that look like unit conversions but are Ohm's-law arithmetic. Named rather
/// than silently converted, because "convert amps to watts" has no answer — and a tool that
/// pretends otherwise is how a wrong number gets a confident tone of voice.
fn not_a_conversion(what: &str) -> Option<String> {
    match what {
        "amp" | "amps" | "a" | "volt" | "volts" | "v" | "ohm" | "ohms" | "watt" | "watts" => {
            Some(format!(
                "\"{what}\" is not a unit you can convert between — volts, amps, watts and ohms \
                 are different kinds of electrical quantity. Use the calculate tool with the \
                 relation you mean: P = V × I, V = I × R, so 500 W at 230 V is 500/230."
            ))
        }
        _ => None,
    }
}

fn is_temp(u: &Unit) -> bool {
    u.1 == "temperature"
}

fn to_kelvin(u: &str, v: f64) -> Option<f64> {
    match u {
        "c" | "celsius" | "centigrade" => Some(v + 273.15),
        "f" | "fahrenheit" => Some((v - 32.0) / 1.8 + 273.15),
        "k" | "kelvin" => Some(v),
        _ => None,
    }
}

fn from_kelvin(u: &str, k: f64) -> Option<f64> {
    match u {
        "c" | "celsius" | "centigrade" => Some(k - 273.15),
        "f" | "fahrenheit" => Some((k - 273.15) * 1.8 + 32.0),
        "k" | "kelvin" => Some(k),
        _ => None,
    }
}

/// Canonical spelling of a unit, following its aliases. Exact match first, then the alias
/// list, then a trailing `s` — which is the only inflection allowed, so "ston" does not become
/// "stone" and "pin" does not become "pint".
fn lookup(u: &str) -> Option<&'static Unit> {
    let want = u.trim().to_lowercase();
    if want.is_empty() {
        return None;
    }
    if let Some(hit) = UNITS.iter().find(|(name, _, _, _, _)| *name == want) {
        return Some(hit);
    }
    if let Some(hit) = UNITS
        .iter()
        .find(|(_, _, _, aliases, _)| aliases.iter().any(|a| *a == want))
    {
        return Some(hit);
    }
    if let Some(stripped) = want.strip_suffix('s') {
        if let Some(hit) = UNITS.iter().find(|(name, _, _, _, _)| *name == stripped) {
            return Some(hit);
        }
        if let Some(hit) = UNITS
            .iter()
            .find(|(_, _, _, aliases, _)| aliases.contains(&stripped))
        {
            return Some(hit);
        }
    }
    None
}

fn unit_list() -> String {
    let mut dims: Vec<&'static str> = Vec::new();
    for (_, dim, _, _, _) in UNITS {
        if !dims.contains(dim) {
            dims.push(dim);
        }
    }
    let mut lines = Vec::new();
    for dim in dims {
        let names: Vec<&str> = UNITS
            .iter()
            .filter(|(_, d, _, _, _)| *d == dim)
            .map(|(n, _, _, _, _)| *n)
            .collect();
        lines.push(format!("{dim}: {}", names.join(" ")));
    }
    lines.join("\n")
}

fn convert(_src: &Sources, arguments: &Json) -> Result<String, String> {
    let from = args::str_arg(arguments, "from");
    let to = args::str_arg(arguments, "to");
    // No `from` at all is the help call, and it is worth having: "what units do you know" is
    // a question a person asks, and the alternative is the model guessing a spelling.
    let (Some(from), Some(to)) = (from, to) else {
        return Ok(format!("Units I can convert between:\n{}", unit_list()));
    };

    let from_u = match lookup(&from) {
        Some(u) => u,
        None => {
            if let Some(advice) = not_a_conversion(&from.to_lowercase()) {
                return Err(advice);
            }
            return Err(format!(
                "I do not know the unit \"{from}\". I can convert: {}",
                unit_list().replace('\n', " · ")
            ));
        }
    };
    let to_u = match lookup(&to) {
        Some(u) => u,
        None => {
            if let Some(advice) = not_a_conversion(&to.to_lowercase()) {
                return Err(advice);
            }
            return Err(format!("I do not know the unit \"{to}\"."));
        }
    };

    let amount = match args::num_arg(arguments, "amount")? {
        Some(v) => (v, String::new()),
        // Naming the assumption instead of hiding it is the rule in `args.rs`: a default the
        // caller did not ask for is allowed only if the answer says so.
        None => (1.0, " (no amount given, so one unit)".to_string()),
    };
    let (amount, assumed) = amount;

    let out = if is_temp(from_u) && is_temp(to_u) {
        let k = to_kelvin(from_u.0, amount).ok_or("internal: temperature table")?;
        from_kelvin(to_u.0, k).ok_or("internal: temperature table")?
    } else if is_temp(from_u) || is_temp(to_u) {
        return Err(format!(
            "\"{}\" is a temperature and \"{}\" is not — you cannot convert between them",
            from_u.0, to_u.0
        ));
    } else if from_u.1 != to_u.1 {
        return Err(format!(
            "\"{}\" is {} and \"{}\" is {} — you cannot convert between them",
            from_u.0, from_u.1, to_u.0, to_u.1
        ));
    } else {
        amount * from_u.2 / to_u.2
    };

    // Which gallon did we just use? Say so whenever the name the caller sent was one of the
    // ambiguous ones, so the note cannot be silently wrong in either direction.
    let mut notes: Vec<&str> = Vec::new();
    if let Some(n) = from_u.4 {
        notes.push(n);
    }
    if let Some(n) = to_u.4 {
        notes.push(n);
    }
    let tail = if notes.is_empty() {
        String::new()
    } else {
        format!(" — {}", notes.join("; "))
    };
    Ok(format!(
        "{amount} {} = {} {}{}{}",
        from_u.0,
        fmt(out),
        to_u.0,
        tail,
        assumed
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use common_json as json;

    fn args(pairs: Vec<(&str, Json)>) -> Json {
        json::object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<Vec<_>>())
    }

    /// These three tools take `&Sources` and never look at it, so the tests hand them inert
    /// URLs. That is the property worth keeping: a tool whose tests need a live Home
    /// Assistant is a tool that cannot be tested offline.
    fn src() -> Sources {
        Sources::new(
            "http://127.0.0.1:1".into(),
            "http://127.0.0.1:1".into(),
            None,
            "http://127.0.0.1:1".into(),
            "http://127.0.0.1:1".into(),
            "http://127.0.0.1:1".into(),
            std::time::Duration::from_secs(1),
        )
    }







    #[test]
    fn defined_conversions_land_on_the_defined_number() {
        let src = src();
        let r = convert(&src, &args(vec![
            ("amount", json::number_from_f64(1.0)),
            ("from", json::string("in")),
            ("to", json::string("mm")),
        ]))
        .unwrap();
        assert_eq!(r, "1 in = 25.4 mm", "an inch is 25.4 mm by definition");

        let r = convert(&src, &args(vec![
            ("amount", json::number_from_f64(1.0)),
            ("from", json::string("lb")),
            ("to", json::string("kg")),
        ]))
        .unwrap();
        assert_eq!(r, "1 lb = 0.453592 kg");

        let r = convert(&src, &args(vec![
            ("amount", json::number_from_f64(60.0)),
            ("from", json::string("mph")),
            ("to", json::string("km/h")),
        ]))
        .unwrap();
        assert_eq!(r, "60 mph = 96.5606 km/h");
    }

    #[test]
    fn the_uk_gallon_is_the_default_and_says_so() {
        let src = src();
        let uk = convert(&src, &args(vec![
            ("amount", json::number_from_f64(1.0)),
            ("from", json::string("gal")),
            ("to", json::string("l")),
        ]))
        .unwrap();
        assert!(uk.contains("4.54609 l"), "{uk}");
        assert!(uk.contains("imperial (UK) gallon"), "{uk}");
        let us = convert(&src, &args(vec![
            ("amount", json::number_from_f64(1.0)),
            ("from", json::string("gal_us")),
            ("to", json::string("l")),
        ]))
        .unwrap();
        assert!(us.contains("3.78541 l"), "{us}");
        assert!(us.contains("US gallon"), "{us}");
    }

    #[test]
    fn temperature_converts_on_the_points_everyone_knows() {
        let src = src();
        let cases = [
            (0.0, "c", "f", "32"),
            (100.0, "c", "f", "212"),
            (20.0, "c", "f", "68"),
            (-40.0, "c", "f", "-40"),
            (37.5, "c", "f", "99.5"),
            (0.0, "c", "k", "273.15"),
        ];
        for (amount, from, to, want) in cases {
            let r = convert(
                &src,
                &args(vec![
                    ("amount", json::number_from_f64(amount)),
                    ("from", json::string(from)),
                    ("to", json::string(to)),
                ]),
            )
            .unwrap();
            assert!(r.ends_with(format!("{want} {to}").as_str()), "{amount} {from} → {r}");
        }
        // -40 either way is the identity every thermometer check uses.
        assert_eq!(
            convert(
                &src,
                &args(vec![
                    ("amount", json::number_from_f64(-40.0)),
                    ("from", json::string("f")),
                    ("to", json::string("c")),
                ])
            )
            .unwrap()
            .split("= ")
            .last()
            .unwrap()
            .split(' ')
            .next()
            .unwrap(),
            "-40"
        );
    }

    #[test]
    fn a_dimension_mismatch_says_which_is_which() {
        let src = src();
        let e = convert(
            &src,
            &args(vec![
                ("amount", json::number_from_f64(1.0)),
                ("from", json::string("kg")),
                ("to", json::string("m")),
            ]),
        )
        .unwrap_err();
        assert_eq!(e, "\"kg\" is mass and \"m\" is length — you cannot convert between them");
        // And the electrical question with no answer gets the formula instead of a number.
        let e = convert(
            &src,
            &args(vec![
                ("amount", json::number_from_f64(5.0)),
                ("from", json::string("amps")),
                ("to", json::string("watts")),
            ]),
        )
        .unwrap_err();
        assert!(e.contains("P = V × I"), "{e}");
    }

    #[test]
    fn an_unknown_spelling_shows_the_menu() {
        let src = src();
        let e = convert(
            &src,
            &args(vec![
                ("amount", json::number_from_f64(1.0)),
                ("from", json::string("furlongs")),
                ("to", json::string("m")),
            ]),
        )
        .unwrap_err();
        assert!(e.contains("furlongs") && e.contains("length:"), "{e}");
        // Plurals and British spellings work; near-misses do not silently resolve.
        assert!(lookup("inches").is_some());
        assert!(lookup("metres").is_some());
        assert!(lookup("ston").is_none(), "must not quietly become stone");
        assert!(lookup("pin").is_none(), "must not quietly become pint");
    }

    #[test]
    fn no_from_is_the_help_call() {
        let src = src();
        let r = convert(&src, &args(vec![])).unwrap();
        assert!(r.contains("temperature: c f k"), "{r}");
        assert!(r.contains("length: nm"), "{r}");
    }

    #[test]
    fn the_clock_tool_labels_every_time_it_prints() {
        let src = src();
        let r = what_time(&src, &args(vec![])).unwrap();
        // Whatever the instant, the answer must name its zone twice: the abbreviation and the
        // offset, plus the UTC rendering. An unlabelled time is the bug this tool exists to fix.
        assert!(r.contains("Europe/London"), "{r}");
        assert!(r.contains("GMT") || r.contains("BST"), "{r}");
        assert!(r.contains("UTC+0") || r.contains("UTC+1"), "{r}");
        assert!(r.starts_with("Mon ") || r.starts_with("Tue ") || r.starts_with("Wed ")
            || r.starts_with("Thu ") || r.starts_with("Fri ") || r.starts_with("Sat ")
            || r.starts_with("Sun "), "{r}");
        // …and never an ISO date, which the speech front end reads with the word "dash" in
        // it. This is the whole reason `nice_date` exists, so it is pinned at the boundary.
        assert!(!r.contains("2026-") && !r.contains("-11-17"), "{r}");
    }

}
