# post-cortex-blobs

The `BlobStore` adapter of post-cortex (docs/ARCHITECTURE.md section 5): the bytes
of the images a module's documents name, kept by their SHA-256 per workspace
(`<workspace>/<sha256>`) on any S3 endpoint, apart from the store of records and
documents. What a blob is (media type, width, height, when it was put) travels as
the object's attributes.
