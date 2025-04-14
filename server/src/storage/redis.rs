use anyhow::{anyhow, Ok, Result};
use base64::{prelude::BASE64_STANDARD, Engine as _};
use deadpool_redis::Connection;
use redis::{cmd, FromRedisValue, Value};
use std::time::{SystemTime, UNIX_EPOCH};

const PAGE_SIZE: u32 = 100;

pub(crate) fn decode<T>(values: Vec<Value>) -> Result<Vec<T>>
where
    T: serde::de::DeserializeOwned,
{
    let strings = values
        .into_iter()
        .map(|v| String::from_redis_value(&v))
        .collect::<redis::RedisResult<Vec<String>>>()?;

    let data = strings
        .into_iter()
        .map(|v| {
            let base64 = v
                .split(":")
                .nth(1)
                .expect("Redis value in wrong delimiter format!");
            let data = BASE64_STANDARD
                .decode(base64)
                .expect("Redis value not base64! ");
            bincode::deserialize(&data)
        })
        .collect::<Result<Vec<T>, _>>()?;

    Ok(data)
}

pub(crate) fn decode_raw(values: Vec<Value>) -> Result<Vec<Vec<u8>>> {
    let strings = values
        .into_iter()
        .map(|v| String::from_redis_value(&v))
        .collect::<redis::RedisResult<Vec<String>>>()?;

    let data = strings
        .into_iter()
        .map(|v| {
            let base64 = v
                .split(":")
                .nth(1)
                .expect("Redis value in wrong delimiter format!");
            let data = BASE64_STANDARD
                .decode(base64)
                .map_err(|err| anyhow!("Redis value not base64!: {err}"))?;
            Ok(data)
        })
        .collect::<Result<Vec<Vec<u8>>>>()?;

    Ok(data)
}

pub(crate) fn to_string(value: &Value) -> Result<String> {
    let string = Vec::<String>::from_redis_value(value)?
        .get(0)
        .ok_or_else(|| anyhow!("Failed"))?
        .to_string();
    Ok(string)
}

pub(crate) fn get_field(value: &Value) -> Result<u64> {
    println!("value: {:?}", value);
    let string = Vec::<String>::from_redis_value(value)?
        .get(0)
        .ok_or_else(|| anyhow!("Failed"))?
        .to_string();
    // let string = values.get(0).ok_or_else(|| anyhow!("asd"))?;

    println!("string: {:}", string);
    let field = string.split(":").nth(0).ok_or_else(|| anyhow!("Failed"))?;
    Ok(field.parse()?)
}

pub(crate) fn create_entry(id: u64, value: Vec<u8>) -> String {
    format!("{}:{}", id, BASE64_STANDARD.encode(&value))
}

pub async fn insert(
    mut connection: Connection,
    queue_key: String,
    queue_metadata_key: String,
    queue_total_index_key: String,
    field_guid: &str,
    value: Vec<u8>,
) -> Result<u64> {
    let message_guid_exists = cmd("HEXISTS")
        .arg(&queue_metadata_key)
        .arg(field_guid)
        .query_async::<u8>(&mut connection)
        .await?;

    if message_guid_exists == 1 {
        let num = cmd("HGET")
            .arg(&queue_metadata_key)
            .arg(field_guid)
            .query_async::<String>(&mut connection)
            .await?;

        return Ok(num.parse().expect("Could not parse redis id"));
    }

    #[rustfmt::skip]
    let value_id = cmd("HINCRBY")
        .arg(&queue_metadata_key) // key (hash)
        .arg("counter")           // field
        .arg(1)                   // increment by 1
        .query_async::<u64>(&mut connection)
        .await?;

    let value_unique = format!("{}:{}", value_id, BASE64_STANDARD.encode(&value));

    #[rustfmt::skip]
    cmd("ZADD")
        .arg(&queue_key)          // key (hash)
        .arg("NX")                // NX: Only add new elements. Don't update already existing elements.
        .arg(value_id)            // order by id (HINCRBY)
        .arg(&value_unique)       // value
        .query_async::<()>(&mut connection)
        .await?;

    #[rustfmt::skip]
    cmd("HSET")
        .arg(&queue_metadata_key)  // key (hash)
        .arg(field_guid)           // field
        .arg(value_id)             // value
        .query_async::<()>(&mut connection)
        .await?;

    #[rustfmt::skip]
    cmd("EXPIRE")
        .arg(&queue_key)           // key (hash)
        .arg(2678400)              // expire in 31 days
        .query_async::<()>(&mut connection)
        .await?;

    #[rustfmt::skip]
    cmd("EXPIRE")           
        .arg(&queue_metadata_key)  // key (hash)
        .arg(2678400)              // expire in 31 days
        .query_async::<()>(&mut connection)
        .await?;

    let time = SystemTime::now();
    let time_in_millis: u64 = time.duration_since(UNIX_EPOCH)?.as_secs();
    #[rustfmt::skip]
    cmd("ZADD")
        .arg(&queue_total_index_key) // key (hash)
        .arg("NX")                   // NX: Only add new elements. Don't update already existing elements.
        .arg(time_in_millis)         // order by time
        .arg(&queue_key)             // value
        .query_async::<()>(&mut connection)
        .await?;

    Ok(value_id)
}

pub async fn remove<T>(
    mut connection: Connection,
    queue_key: String,
    queue_metadata_key: String,
    queue_total_index_key: String,
    field_guids: Vec<String>,
) -> Result<Vec<T>>
where
    T: serde::de::DeserializeOwned,
{
    let mut removed_values = Vec::new();

    for guid in field_guids {
        let value_id: Option<String> = cmd("HGET")
            .arg(&queue_metadata_key)
            .arg(&guid)
            .query_async(&mut connection)
            .await?;

        if let Some(value_id) = value_id.clone() {
            // retrieving the message
            let values = cmd("ZRANGE")
                .arg(&queue_key)
                .arg(&value_id)
                .arg(&value_id)
                .arg("BYSCORE")
                .arg("LIMIT")
                .arg(0)
                .arg(1)
                .query_async::<Option<Vec<Value>>>(&mut connection)
                .await?;

            // delete the message
            cmd("ZREMRANGEBYSCORE")
                .arg(&queue_key)
                .arg(&value_id)
                .arg(&value_id)
                .query_async::<()>(&mut connection)
                .await?;

            // delete the guid from the cache
            cmd("HDEL")
                .arg(&queue_metadata_key)
                .arg(&guid)
                .query_async::<()>(&mut connection)
                .await?;

            if let Some(values) = values {
                for data in decode(values)? {
                    removed_values.push(data);
                }
            }
        }
    }

    if cmd("ZCARD")
        .arg(&queue_key)
        .query_async::<u64>(&mut connection)
        .await?
        == 0
    {
        cmd("DEL")
            .arg(&queue_key)
            .query_async::<()>(&mut connection)
            .await?;

        cmd("DEL")
            .arg(&queue_metadata_key)
            .query_async::<()>(&mut connection)
            .await?;

        cmd("ZREM")
            .arg(&queue_total_index_key)
            .arg(&queue_key)
            .query_async::<()>(&mut connection)
            .await?;
    }

    Ok(removed_values)
}

pub async fn get_values(
    mut connection: Connection,
    queue_key: String,
    queue_lock_key: String,
    stop_index: i32,
) -> Result<Vec<Value>> {
    let values_sort = format!("({}", stop_index);

    let locked = cmd("GET")
        .arg(&queue_lock_key)
        .query_async::<Option<String>>(&mut connection)
        .await?;

    // if there is a queue lock key on, due to persist of message.
    if locked.is_some() {
        return Ok(Vec::new());
    }

    let values = cmd("ZRANGE")
        .arg(queue_key.clone())
        .arg(values_sort.clone())
        .arg("+inf")
        .arg("BYSCORE")
        .arg("LIMIT")
        .arg(0)
        .arg(PAGE_SIZE)
        // .arg("WITHSCORES")
        .query_async::<Vec<Value>>(&mut connection)
        .await?;

    Ok(values)
}

async fn get_first(
    connection: &mut Connection,
    queue_key: String,
    queue_lock_key: String,
) -> Result<Value> {
    let locked = cmd("GET")
        .arg(&queue_lock_key)
        .query_async::<Option<String>>(connection)
        .await?;

    if locked.is_some() {
        return Err(anyhow!("Failed to get first value: queue is locked."));
    }

    // Get value at index 0
    let value = cmd("ZRANGE")
        .arg(queue_key)
        .arg(0)
        .arg(0)
        .query_async::<Vec<Value>>(connection)
        .await?
        .get(0)
        .ok_or_else(|| anyhow!("Failed to get first value: empty value."))?
        .clone();

    Ok(value)
}

async fn update_value(
    connection: &mut Connection,
    queue_key: String,
    field_id: u64,
    new_value: Vec<u8>,
) -> Result<bool> {
    let message_guid_exists = cmd("ZRANGEBYSCORE")
        .arg(&queue_key)
        .arg(field_id)
        .arg(field_id)
        .query_async::<Vec<Value>>(connection)
        .await?;

    if message_guid_exists.is_empty() {
        return Err(anyhow!("Failed to update non-existent value."));
    }

    let old_value = message_guid_exists
        .get(0)
        .ok_or_else(|| anyhow!("failed"))?;

    let old_entry = to_string(old_value)?;
    let new_entry = create_entry(field_id, new_value);

    // Remove old value
    let removed = cmd("ZREM")
        .arg(&queue_key)
        .arg(old_entry)
        .query_async::<u64>(connection)
        .await?;
    if removed != 1 {
        return Err(anyhow!("Failed to remove value!"));
    }

    // Set new value
    let added = cmd("ZADD")
        .arg(queue_key)
        .arg(field_id)
        .arg(new_entry)
        .query_async::<u64>(connection)
        .await?;
    if added != 1 {
        return Err(anyhow!("Failed to add value!"));
    }

    (added == removed)
        .then_some(true)
        .ok_or_else(|| anyhow!("Update failed!"))
}

/// Take part of redis value out and remove
pub async fn take_values(
    mut connection: Connection,
    queue_key: String,
    queue_lock_key: String,
    data_size: usize,
) -> Result<Vec<u8>> {
    let first = get_first(&mut connection, queue_key.clone(), queue_lock_key).await?;
    let field_id = get_field(&first)?;
    let mut value = decode_raw(vec![first.clone()])?[0].clone();
    let rest = value.split_off(data_size);

    let updated = update_value(&mut connection, queue_key, field_id, rest).await?;
    if !updated {
        return Err(anyhow!("Failed to update value."));
    }

    Ok(value)
}
