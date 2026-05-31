create table evaluation_cache (
  flake_path_digest blob,
  flake_path_name text,
  flake_hash_sri text,
  node_name text,

  output_path_digest blob not null,
  output_path_name text not null,
  
  primary key (flake_path_digest, flake_path_name, flake_hash_sri, node_name)
) strict;
