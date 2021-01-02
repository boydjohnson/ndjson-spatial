<h1 align="center">ndjson</h1>

> Command line tool for creating, filtering, and processing ndjson

[![boydjohnson](https://circleci.com/gh/boydjohnson/ndjson-spatial/tree/master.svg?style=shield)](https://circleci.com/gh/boydjohnson/ndjson-spatial/tree/master)

### 🏠 [Homepage](https://github.com/boydjohnson/ndjson-spatial)

## Usage

```sh
SUBCOMMANDS:
    agg           Aggregatation commands on a grouped-by key
    filter        returns only json that matches filter expression
    from-json     Converts json to ndjson
    join          joins json file to ndjson stream
    pick-field    picks a field from all of the ndjson objects
```

## Use-case
### From-Json
Convert JSON to NDJSON

```sh
cat <file.json> | ndjson from-json <selector>
```

### Example

```sh
cat senators.json | ndjson from-json d.objects
```

### Filter

Filter NDJSON

```sh
cat <file.ndjson> | ndjson filter <selector>
```

### Example
Filter where field is equal to a value

```sh
cat <complaints.ndjson> | ndjson filter 'd.product == Virtual Currency'
```

### Join

Join two ndjson streams, one from a file and one from stdin.

```sh
cat <file1.ndjson> | ndjson join --ref <file2.ndjson> --ref-fields <selector>... --stream-fields <selector>...
```

### Example
Join on state and county names.

```sh
cat state_county.ndjson | ndjson join -ref state_county_population.ndjson --ref-fields d.state d.county --stream-fields d.state d.county
```

### Agg
Perform a group-by operation and then get Sum, Min, Max, or Count information.

```sh
cat <file.ndjson> | ndjson agg --group-by <selector> --agg <count | sum | min | max> <selector>
```

### Example
Get the sum of some numeric field grouped by another field.

```sh
cat state_county.ndjson | ndjson agg --group-by d.state --agg sum d.population 
```

### Pick-Field
Get a single value from all NDJSON objects

```sh
cat <file.ndjson> | ndjson pick-field <selector>
```

### Example

Use sort and uniq to get the count of distinct values of some field

```sh
cat senators.ndjson | ndjson pick-field d.person.gender | sort | uniq -c | sort -n
```

## Build

```sh
cargo build -Z unstable-options --profile=release-ndjson
```

## Run tests

```sh
cargo test
```

## Author

👤 **Boyd Johnson**

* Website: https://boydjohnson.dev
* Twitter: [@boydjohnson04](https://twitter.com/boydjohnson04)
* Github: [@boydjohnson](https://github.com/boydjohnson)

## 🤝 Contributing

Contributions, issues and feature requests are welcome!<br />Feel free to check [issues page](https://github.com/boydjohnson/ndjson-spatial/issues). 

## Show your support

Give a ⭐️ if this project helped you!

## 📝 License

<p>
  <a href="https://www.apache.org/licenses/LICENSE-2.0.txt" target="_blank">
    <img alt="License: Apache--2.0" src="https://img.shields.io/badge/License-Apache--2.0-yellow.svg" />
  </a>
</p>

Copyright © 2021 [Boyd Johnson](https://github.com/boydjohnson).<br />
This project is [Apache--2.0](https://www.apache.org/licenses/LICENSE-2.0.txt) licensed.

***
_This README was generated with ❤️ by [readme-md-generator](https://github.com/kefranabg/readme-md-generator)_