//! Generic D-Bus helpers: everything is converted to serde_json::Value so the
//! services stay small and never need generated proxies.

use std::collections::HashMap;

use anyhow::Result;
use serde_json::{Map, Value};
use zbus::{Connection, MatchRule, MessageStream, OwnedObjectPath, OwnedVariant, Variant};

pub fn to_json(v: &Variant<'_>) -> Value {
    match v {
        Variant::U8(x) => Value::from(*x),
        Variant::Bool(x) => Value::from(*x),
        Variant::I16(x) => Value::from(*x),
        Variant::U16(x) => Value::from(*x),
        Variant::I32(x) => Value::from(*x),
        Variant::U32(x) => Value::from(*x),
        Variant::I64(x) => Value::from(*x),
        Variant::U64(x) => Value::from(*x),
        Variant::F64(x) => Value::from(*x),
        Variant::Str(s) => Value::from(s.as_str()),
        Variant::Signature(s) => Value::from(s.to_string()),
        Variant::ObjectPath(p) => Value::from(p.as_str()),
        Variant::Variant(b) => to_json(b),
        Variant::Array(a) => {
            let items = a.inner();
            if a.element_signature().to_string() == "y" {
                // Byte arrays (SSIDs, image data) → lossy string.
                let bytes: Vec<u8> = items.iter().filter_map(|i| if let Variant::U8(b) = i { Some(*b) } else { None }).collect();
                Value::from(String::from_utf8_lossy(&bytes).into_owned())
            } else {
                Value::Array(items.iter().map(to_json).collect())
            }
        }
        Variant::Dict(d) => {
            let mut m = Map::new();
            for (k, val) in d.iter() {
                let key = match to_json(k) {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                m.insert(key, to_json(val));
            }
            Value::Object(m)
        }
        Variant::Structure(s) => Value::Array(s.fields().iter().map(to_json).collect()),
        #[cfg(unix)]
        Variant::Fd(_) => Value::Null,
    }
}

pub fn map_to_json(m: &HashMap<String, OwnedVariant>) -> Value {
    Value::Object(m.iter().map(|(k, v)| (k.clone(), to_json(v))).collect())
}

pub async fn get_all(conn: &Connection, dest: &str, path: &str, iface: &str) -> Result<Value> {
    let msg = conn
        .call_method(Some(dest), path, Some("org.freedesktop.DBus.Properties"), "GetAll", &(iface,))
        .await?;
    let body = msg.body();
    let m: HashMap<String, OwnedVariant> = body.deserialize()?;
    Ok(map_to_json(&m))
}

pub async fn get(conn: &Connection, dest: &str, path: &str, iface: &str, prop: &str) -> Result<Value> {
    let msg = conn
        .call_method(Some(dest), path, Some("org.freedesktop.DBus.Properties"), "Get", &(iface, prop))
        .await?;
    let body = msg.body();
    let v: OwnedVariant = body.deserialize()?;
    Ok(to_json(&v))
}

pub async fn set<'a>(conn: &Connection, dest: &str, path: &str, iface: &str, prop: &str, val: Variant<'a>) -> Result<()> {
    conn.call_method(Some(dest), path, Some("org.freedesktop.DBus.Properties"), "Set", &(iface, prop, val)).await?;
    Ok(())
}

pub async fn managed_objects(conn: &Connection, dest: &str, path: &str) -> Result<Value> {
    let msg = conn.call_method(Some(dest), path, Some("org.freedesktop.DBus.ObjectManager"), "GetManagedObjects", &()).await?;
    let body = msg.body();
    let m: HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedVariant>>> = body.deserialize()?;
    let mut out = Map::new();
    for (p, ifaces) in m {
        let mut im = Map::new();
        for (iname, props) in ifaces {
            im.insert(iname, map_to_json(&props));
        }
        out.insert(p.as_str().to_string(), Value::Object(im));
    }
    Ok(Value::Object(out))
}

/// Stream of every signal emitted by `sender` (well-known name).
pub async fn signals_from(conn: &Connection, sender: &str) -> Result<MessageStream> {
    let rule = MatchRule::builder().msg_type(zbus::message::Type::Signal).sender(sender.to_string()).build()?;
    Ok(MessageStream::for_match_rule(rule, conn, None).await?)
}

pub async fn signals_rule(conn: &Connection, rule: MatchRule<'_>) -> Result<MessageStream> {
    Ok(MessageStream::for_match_rule(rule, conn, None).await?)
}

pub fn s(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
