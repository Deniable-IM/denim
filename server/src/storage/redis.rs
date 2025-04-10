use anyhow::{anyhow, Result};
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
