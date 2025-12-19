create table inspection_blobs (
  id integer primary key autoincrement,
  json_value blob not null unique,
  schema_version integer not null
) strict;

create table inspection_cache (
  store_path text,
  hash text,
  blob_id integer references inspection_blobs (id) not null,
  primary key (store_path, hash)
) strict;

drop table cached_inspection;

drop table hive_inspection;
