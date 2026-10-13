const db = await Ende.openKv("http://localhost:4545/kv_remote_authorize");
console.log("opened");
db.close();
