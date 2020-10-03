shm-cached
==========
An HTTP cache specifically optimised for Shimmie galleries

- Designed to run as a cluster of image mirrors who coordinate to
  share the load
- Gets the load-balancing config by looking at shimmie's config
  database
- Automatically re-balances whenever the config changes

Why
---
- When you have 100GB of images, and 4 x 32GB caches, this allows
  each host to be responsible for 25GB of data (100% cache hit rate)
  instead of random / round-robin balacing (~30% cache hit rate)
- The algorithm allows each image to be owned by a primary and
  secondary cache (so each host would be primary for 25GB and
  secondary for 25GB) - this means that if a host has a hardware
  failure, it can be pulled out, and the secondary hosts will be
  warmed up already (even if they need to go to disk instead of RAM)
- When the balancing config changes, hosts redirect load to the
  new owner instead of lowering their own cache hit rate

TODO
----
- Integrate with the `image_hash_bans` table to automatically nuke
  things from the cache whenever an image is banned from the backend

How to use
----------
- Run this software on the three hosts `mirror-{a,b,c}.mysite.com`
- Set `image_ilink` to something like:
  `https://mirror-{a=5,b=5,c=10}.mysite.com/_images/$hash/$id-$tags.$ext`
- Shimmie then generates links like
  `https://mirror-c.mysite.com/_images/ab25bc2/42-tagme.jpg`
- If the user visits the above URL, then
  - `mirror-c` checks if it is responsible for this file
  - if it is, and it has a copy, it sends it
  - if it is, and it doesn't have a copy, it fetches it from the backend
  - if it isn't responsible (eg the load-balancing config has changed
    recently), it sends a HTTP redirect to the current owner
