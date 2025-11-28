create table hive_inspection (
        id integer primary key autoincrement,
        json_value text not null unique
) strict;

create table cached_inspection (
        store_path text,
        hash text,

        inspection_id integer references hive_inspection(id) not null,

        primary key (store_path, hash)
) strict;
