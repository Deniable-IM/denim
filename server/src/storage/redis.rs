use anyhow::Result;
use deadpool_redis::Connection;
use redis::cmd;
use std::time::{SystemTime, UNIX_EPOCH};

const PAGE_SIZE: u32 = 100;

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

    #[rustfmt::skip]
    cmd("ZADD")
        .arg(&queue_key)          // key (hash)
        .arg("NX")                // NX: Only add new elements. Don't update already existing elements.
        .arg(value_id)            // order by id (HINCRBY)
        .arg(&value)              // value
        .query_async::<()>(&mut connection)
        .await?;

    #[rustfmt::skip]
    cmd("HSET")
        .arg(&queue_metadata_key)  // key (hash)
        .arg(field_guid)                // field
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
        let message_id: Option<String> = cmd("HGET")
            .arg(&queue_metadata_key)
            .arg(&guid)
            .query_async(&mut connection)
            .await?;

        if let Some(msg_id) = message_id.clone() {
            // retrieving the message
            let envelope = cmd("ZRANGE")
                .arg(&queue_key)
                .arg(&msg_id)
                .arg(&msg_id)
                .arg("BYSCORE")
                .arg("LIMIT")
                .arg(0)
                .arg(1)
                .query_async::<Option<Vec<Vec<u8>>>>(&mut connection)
                .await?;

            // delete the message
            cmd("ZREMRANGEBYSCORE")
                .arg(&queue_key)
                .arg(&msg_id)
                .arg(&msg_id)
                .query_async::<()>(&mut connection)
                .await?;

            // delete the guid from the cache
            cmd("HDEL")
                .arg(&queue_metadata_key)
                .arg(&guid)
                .query_async::<()>(&mut connection)
                .await?;

            if let Some(envel) = envelope {
                removed_values.push(bincode::deserialize(&envel[0])?);
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
) -> Result<Vec<Vec<u8>>> {
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
        .arg("WITHSCORES")
        .query_async::<Vec<Vec<u8>>>(&mut connection)
        .await?;

    Ok(values.clone())
}
