-- migrate schema version to be semver instead of an int

drop table inspection_cache;
drop table inspection_blobs;

create table inspection_blobs (
  id integer primary key autoincrement,
  json_value blob not null unique,
  schema_version text not null
) strict;

create table inspection_cache (
  store_path_digest blob,
  store_path_name text,
  hash_sri text,

  blob_id integer references inspection_blobs (id) not null,

  primary key (store_path_digest, store_path_name, hash_sri)
) strict;
