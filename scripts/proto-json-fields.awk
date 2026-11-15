# Prints every JSON-shaped field of a .proto file (a google.protobuf Struct,
# Value or ListValue, or a string named *json*) as "<file> <message>.<field>",
# for the grpc_json_fields check of scripts/check-architecture.sh.
/^[[:space:]]*\/\// { next }
/^[[:space:]]*(message|enum|oneof|service)[[:space:]]+[A-Za-z0-9_]+[[:space:]]*\{/ {
  match($0, /(message|enum|oneof|service)[ \t]+[A-Za-z0-9_]+/)
  split(substr($0, RSTART, RLENGTH), p, /[ \t]+/)
  depth++; kind[depth] = p[1]; name[depth] = p[2]; next
}
/^[[:space:]]*extend[[:space:]]+[A-Za-z0-9_.]+[[:space:]]*\{/ {
  depth++; kind[depth] = "extend"; name[depth] = ""; next
}
/^[[:space:]]*\}/ { depth--; next }
/google\.protobuf\.(Struct|Value|ListValue)|string [a-z_]*json[a-z_]* *=/ {
  m = ""; for (i = depth; i > 0; i--) if (kind[i] == "message") { m = name[i]; break }
  match($0, /[a-z_0-9]+ *= *[0-9]+/); f = substr($0, RSTART, RLENGTH); sub(/ *=.*/, "", f)
  n = FILENAME; sub(/.*\//, "", n)
  print n " " m "." f
}
